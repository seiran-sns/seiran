//! seiran-server — 全バックエンドロールを内包する統合バイナリ。
//!
//! `--role`（または環境変数 `SEIRAN_ROLE`）で起動時の役割を切り替える。
//! 引数なしで起動すると `all`（全ロールを1プロセスで実行）になり、
//! 小規模サーバー向けの単一コンテナ構成として使える。
//!
//! | ロール | 内容 | HTTP |
//! |---|---|---|
//! | `all`（既定） | api + federation を1ポートに合流、worker と firehose を同時起動 | `PORT`（既定 3000） |
//! | `api` | REST API / 認証 / タイムライン / XRPC | `PORT`（既定 3000） |
//! | `federation` | ActivityPub Inbox / WebFinger / Actor / Outbox | `FEDERATION_INBOX_PORT`（既定 3001） |
//! | `worker` | 非同期ジョブ実行エンジン | なし |
//! | `firehose` | Bluesky Firehose リスナー | なし |
//!
//! 大規模サーバーでは同じイメージを `--role` 違いで複数コンテナ起動し、
//! ワーカー負荷分散などのスケールアウトを行う。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use seiran_common::atp::AtpCommitService;
use seiran_common::did_provision::DidProvisioner;
use seiran_common::repository::{
    InstanceDomainRepository, PgActorRepository, PgBlockRepository, PgFollowRepository,
    PgHashtagRepository, PgInstanceDomainRepository, PgListRepository, PgNotificationRepository,
    PgPostRepository, PgReactionRepository, PgRemoteEmojiRepository, PgSiteSettingsRepository,
};
use seiran_common::{
    ap::ApClient, db::recommended_max_connections, get_db_pool, resolve_local_domain,
    run_migrations, CancellationToken, DeliveryConfig, FollowExecConfig, InboxContext, JobQueue,
    LocalDomain, Messaging, Secrets, SecretsFile, StreamHub, WorkerConfig,
    DEFAULT_MAX_CONCURRENT_JOBS,
};
use sqlx::PgPool;

/// `FollowExecConfig`（`Job::FollowImportProcess` ジョブ用）を組み立てる。standalone worker
/// と `all` ロール埋め込み worker の両方から呼ばれる共通ヘルパー。
fn build_follow_exec_config(
    pool: &PgPool,
    local_domain: &LocalDomain,
    ap_private_key_pem: Option<String>,
    stream_hub: Arc<StreamHub>,
    atp_service: Arc<AtpCommitService>,
) -> FollowExecConfig {
    FollowExecConfig {
        actors: Arc::new(PgActorRepository::new(pool.clone())),
        follows: Arc::new(PgFollowRepository::new(pool.clone())),
        blocks: Arc::new(PgBlockRepository::new(pool.clone())),
        notifications: Arc::new(PgNotificationRepository::new(pool.clone())),
        atp_service,
        stream_hub,
        local_domain: local_domain.clone(),
        ap_private_key_pem: ap_private_key_pem.unwrap_or_default(),
    }
}

/// `InboxContext`（InboundActivityProcess ジョブ用）を組み立てる。
/// standalone worker と `all` ロール埋め込み worker の両方から呼ばれる共通ヘルパー。
fn build_inbox_context(
    pool: &PgPool,
    local_domain: &LocalDomain,
    ap_private_key_pem: Option<String>,
    stream_hub: Arc<StreamHub>,
    queue: Arc<dyn JobQueue>,
    atp_service: Arc<AtpCommitService>,
) -> InboxContext {
    InboxContext {
        db_pool: pool.clone(),
        actor_repo: Arc::new(PgActorRepository::new(pool.clone())),
        follow_repo: Arc::new(PgFollowRepository::new(pool.clone())),
        block_repo: Arc::new(PgBlockRepository::new(pool.clone())),
        post_repo: Arc::new(PgPostRepository::new(pool.clone())),
        reaction_repo: Arc::new(PgReactionRepository::new(pool.clone())),
        notification_repo: Arc::new(PgNotificationRepository::new(pool.clone())),
        hashtag_repo: Arc::new(PgHashtagRepository::new(pool.clone())),
        remote_emoji_repo: Arc::new(PgRemoteEmojiRepository::new(pool.clone())),
        list_repo: Arc::new(PgListRepository::new(pool.clone())),
        local_domain: local_domain.clone(),
        ap_private_key_pem: ap_private_key_pem.unwrap_or_default(),
        stream_hub,
        queue,
        atp_service,
    }
}

/// `instance_domain` テーブルから自ホストドメインを解決する。`.env`の`LOCAL_DOMAIN`が
/// 設定されていれば、DBが未確定の場合の後方互換パス（自動移行）に使う。
async fn resolve_local_domain_from_env(pool: &PgPool) -> LocalDomain {
    let instance_domain: Arc<dyn InstanceDomainRepository> =
        Arc::new(PgInstanceDomainRepository::new(pool.clone()));
    let legacy_env_domain = std::env::var("LOCAL_DOMAIN").ok();
    resolve_local_domain(instance_domain.as_ref(), legacy_env_domain).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    All,
    Api,
    Federation,
    Worker,
    Firehose,
}

impl Role {
    /// `--role=xxx` / `--role xxx` / `SEIRAN_ROLE` の順で解決する。いずれも無ければ `all`。
    fn resolve() -> Self {
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            if let Some(value) = arg.strip_prefix("--role=") {
                return Self::from_name(value);
            }
            if arg == "--role" {
                if let Some(value) = args.next() {
                    return Self::from_name(&value);
                }
            }
        }
        if let Ok(value) = std::env::var("SEIRAN_ROLE") {
            if !value.is_empty() {
                return Self::from_name(&value);
            }
        }
        Role::All
    }

    fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "all" => Role::All,
            "api" => Role::Api,
            "federation" | "inbox" => Role::Federation,
            "worker" => Role::Worker,
            "firehose" | "atp-repo" => Role::Firehose,
            other => {
                tracing::warn!(
                    "[seiran-server] 不明なロール '{}' → 'all' で起動します",
                    other
                );
                Role::All
            }
        }
    }
}

/// `shutdown` が発火したら新しい接続の受け付けを止め、処理中のリクエストが終わってから戻る。
/// WebSocket はハンドラが同じ合図で閉じる（`AppState::shutdown`）。
async fn serve(
    app: axum::Router,
    port: u16,
    shutdown: CancellationToken,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("[seiran-server] リッスン開始: http://{}", addr);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown.cancelled_owned())
    .await?;
    tracing::info!("[seiran-server] HTTP の受け付けを止めました。");
    Ok(())
}

/// SIGTERM（`docker stop`）か SIGINT（Ctrl-C）を受けたら `shutdown` を発火する。
fn spawn_signal_handler(shutdown: CancellationToken) {
    tokio::spawn(async move {
        let mut sigterm =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(sigterm) => sigterm,
                Err(e) => {
                    tracing::error!("[seiran-server] SIGTERM を待ち受けられません: {}", e);
                    return;
                }
            };
        tokio::select! {
            _ = sigterm.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
        tracing::info!(
            "[seiran-server] 停止の合図を受けました。処理中の仕事を終えてから終了します。"
        );
        shutdown.cancel();
    });
}

fn env_port(key: &str, default: u16) -> u16 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// ログ・パニックフック・TLS 実装・`.env` を初期化する。
fn init_process() {
    // `RUST_LOG`（例: `RUST_LOG=debug`, `RUST_LOG=seiran_common=debug,info`）でレベル制御。
    // 未設定時は info レベル。
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // blurhash 0.2.x のオフバイワンバグによる既知パニックを stderr に出力しない。
    // catch_unwind で回復済みのため、ログノイズを抑制するだけで動作は正常。
    std::panic::set_hook(Box::new(|info| {
        let msg = info.to_string();
        if !msg.contains("blurhash") {
            tracing::error!("{}", msg);
        }
    }));

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let _ = dotenvy::dotenv();
}

/// DB に接続し、マイグレーションを適用する。`worker_extra` は同一プールを使う worker の
/// 同時実行数（接続数の見積もりに上乗せする）。
async fn connect_db(worker_extra: u32) -> Result<PgPool, Box<dyn std::error::Error>> {
    let pool = get_db_pool(recommended_max_connections(worker_extra)).await?;
    tracing::info!("[seiran-server] DB 接続完了");
    // instance_domain含む全テーブルがこの時点で必要（resolve_local_domain_from_env
    // より前に完了させる。未適用のままだと instance_domain 読み取りが失敗し、
    // local_domain が常に未確定のまま起動してしまう）。Firehose/Federation単独起動
    // でも必ずマイグレーション済みの状態にするため、ロールに関わらずここで実行する
    // （sqlxのマイグレーションは冪等・アドバイザリロック付きのため複数プロセス
    // 同時実行でも安全）。
    run_migrations(&pool).await?;
    tracing::info!("[seiran-server] マイグレーション適用完了");
    Ok(pool)
}

/// 連合用 HTTP クライアント（非公開IP拒否つき）。
fn build_http_client() -> Result<Arc<reqwest::Client>, Box<dyn std::error::Error>> {
    Ok(Arc::new(
        seiran_common::net::federation_client_builder()
            .user_agent("seiran-federation/0.1.0")
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()?,
    ))
}

fn delivery_config(local_domain: &LocalDomain, secrets: &Secrets) -> DeliveryConfig {
    DeliveryConfig {
        local_domain: local_domain.clone(),
        ap_private_key_pem: secrets.ap_private_key_pem.clone(),
        ap_public_key_pem: secrets.ap_public_key_pem.clone(),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_process();

    let role = Role::resolve();
    tracing::info!("[seiran-server] ロール: {:?}", role);

    let shutdown = CancellationToken::new();
    spawn_signal_handler(shutdown.clone());
    // ジョブキュー・リアルタイム配信・subscribeRepos のイベントをプロセス間でつなぐ。
    // 無ければプロセス内で完結する（`all` を1つだけ動かす構成でのみ成り立つ）。
    let redis_url = std::env::var("REDIS_URL").ok().filter(|s| !s.is_empty());

    if role == Role::Worker {
        return run_standalone_worker(redis_url.as_deref(), shutdown).await;
    }

    // ── 共有リソース（プロセス内で一度だけ生成し各ロールへ渡す）──
    let secrets = Arc::new(SecretsFile::from_env().load_or_create()?);
    tracing::info!("[seiran-server] シークレット読み込み完了");

    // Role::All は埋め込みworkerもこのプールを共有するため、その同時実行数分を上乗せする。
    // Api/Federation/Firehoseはworkerを持たないためHTTP見積もり＋バッファのみで足りる。
    let worker_extra = if role == Role::All {
        DEFAULT_MAX_CONCURRENT_JOBS as u32
    } else {
        0
    };
    let pool = connect_db(worker_extra).await?;
    let http_client = build_http_client()?;
    let local_domain = resolve_local_domain_from_env(&pool).await;
    let messaging = Messaging::connect(redis_url.as_deref(), shutdown.clone()).await?;

    let shared = SharedResources {
        pool,
        secrets,
        http_client,
        local_domain,
        messaging,
    };
    match role {
        Role::Firehose => {
            let did_provisioner = build_did_provisioner(&shared);
            seiran_atp_repo::run(
                shared.pool,
                shared.http_client,
                shared.messaging.stream_hub,
                shared.messaging.job_queue,
                did_provisioner,
                shutdown,
            )
            .await;
        }
        Role::Api => {
            let state = seiran_api::init_state(
                shared.pool,
                shared.secrets,
                shared.http_client,
                shared.local_domain,
                shared.messaging,
            )
            .await;
            seiran_api::spawn_startup_tasks(&state);
            seiran_api::spawn_gc_tasks(&state);
            serve(seiran_api::router(state), env_port("PORT", 3000), shutdown).await?;
        }
        Role::Federation => {
            let state = seiran_federation_inbox::init_state(
                shared.pool,
                &shared.secrets,
                shared.http_client,
                shared.local_domain,
                shared.messaging.stream_hub,
                shared.messaging.job_queue,
            );
            serve(
                seiran_federation_inbox::router(state),
                env_port("FEDERATION_INBOX_PORT", 3001),
                shutdown,
            )
            .await?;
        }
        Role::All => run_all(shared).await?,
        Role::Worker => unreachable!("worker は先頭で分岐済み"),
    }

    tracing::info!("[seiran-server] 終了します。");
    Ok(())
}

/// firehose ロール単独で動かす DID 発行の定時処理（api ロールと同じ設定を DB から読む）。
fn build_did_provisioner(shared: &SharedResources) -> Arc<DidProvisioner> {
    let atp_service = Arc::new(AtpCommitService::new(
        shared.pool.clone(),
        Arc::clone(&shared.messaging.atp_events),
        Arc::clone(&shared.http_client),
        shared.local_domain.clone(),
    ));
    Arc::new(DidProvisioner {
        pool: shared.pool.clone(),
        http: Arc::clone(&shared.http_client),
        atp_service,
        site_settings: Arc::new(PgSiteSettingsRepository::new(shared.pool.clone())),
        local_domain: shared.local_domain.clone(),
        server_rotation_key_pem: shared.secrets.atproto_private_key_pem.clone(),
        job_queue: Arc::clone(&shared.messaging.job_queue),
    })
}

/// worker 以外のロールで共有するプロセス内リソース。
struct SharedResources {
    pool: PgPool,
    secrets: Arc<Secrets>,
    http_client: Arc<reqwest::Client>,
    local_domain: LocalDomain,
    messaging: Messaging,
}

/// standalone worker ロール。
///
/// worker も BskyVideoPoll 等 DB アクセスが必要なジョブを扱うため、単独起動時も
/// DB に接続する。
/// AP 配送ジョブ（ApDelivery）が署名に AP 鍵を使うため、シークレットも読み込む。
async fn run_standalone_worker(
    redis_url: Option<&str>,
    shutdown: CancellationToken,
) -> Result<(), Box<dyn std::error::Error>> {
    let secrets = SecretsFile::from_env().load_or_create()?;
    tracing::info!("[seiran-server] シークレット読み込み完了");
    // standalone worker はHTTPを持たず、WorkerEngineの同時実行数分だけDBを使う。
    let pool = connect_db(DEFAULT_MAX_CONCURRENT_JOBS as u32).await?;
    let http_client = build_http_client()?;
    let worker_local_domain = resolve_local_domain_from_env(&pool).await;
    let delivery = delivery_config(&worker_local_domain, &secrets);
    // REDIS_URL があれば api/federation プロセスとキューを共有し、ここで起きたイベント
    // （受信した投稿・コミット）も api プロセスの WebSocket 接続へ届く。
    let messaging = Messaging::connect(redis_url, shutdown.clone()).await?;
    let atp_service = Arc::new(AtpCommitService::new(
        pool.clone(),
        Arc::clone(&messaging.atp_events),
        Arc::clone(&http_client),
        worker_local_domain.clone(),
    ));
    let inbox = build_inbox_context(
        &pool,
        &worker_local_domain,
        secrets.ap_private_key_pem.clone(),
        Arc::clone(&messaging.stream_hub),
        Arc::clone(&messaging.job_queue),
        Arc::clone(&atp_service),
    );
    let follow_exec = build_follow_exec_config(
        &pool,
        &worker_local_domain,
        secrets.ap_private_key_pem.clone(),
        Arc::clone(&messaging.stream_hub),
        Arc::clone(&atp_service),
    );
    seiran_federation_worker::run(
        WorkerConfig {
            queue: messaging.job_queue,
            pool,
            ap_client: Arc::new(ApClient::new(http_client)),
            delivery,
            inbox: Some(inbox),
            follow_exec: Some(follow_exec),
            atp_service,
            encryption_key: secrets.encryption_key_bytes(),
        },
        shutdown,
    )
    .await;
    tracing::info!("[seiran-server] 終了します。");
    Ok(())
}

/// `all` ロール: api + federation を1ポートに合流し、firehose と worker を同一プロセスで起動する。
/// 停止の合図を受けたら、HTTP・worker・firehose がそれぞれ手持ちの処理を終えるのを待って戻る。
async fn run_all(shared: SharedResources) -> Result<(), Box<dyn std::error::Error>> {
    let SharedResources {
        pool,
        secrets,
        http_client,
        local_domain,
        messaging,
    } = shared;
    let shutdown = messaging.shutdown.clone();
    let job_queue = Arc::clone(&messaging.job_queue);

    // api ロール
    let api_state = seiran_api::init_state(
        pool.clone(),
        Arc::clone(&secrets),
        Arc::clone(&http_client),
        local_domain.clone(),
        messaging,
    )
    .await;
    seiran_api::spawn_startup_tasks(&api_state);
    seiran_api::spawn_gc_tasks(&api_state);

    // federation ロール（ストリーミングハブ・ジョブキューを api/worker と共有する）
    let inbox_state = seiran_federation_inbox::init_state(
        pool.clone(),
        &secrets,
        Arc::clone(&http_client),
        local_domain.clone(),
        Arc::clone(&api_state.stream_hub),
        Arc::clone(&job_queue),
    );

    // firehose リスナーをバックグラウンド起動（stream_hub を共有して WebSocket 配信）
    let firehose = tokio::spawn(seiran_atp_repo::run(
        pool.clone(),
        Arc::clone(&http_client),
        Arc::clone(&api_state.stream_hub),
        Arc::clone(&job_queue),
        Arc::clone(&api_state.did_provisioner),
        shutdown.clone(),
    ));

    // worker をバックグラウンド起動（api ロールと同じ ApClient / JobQueue / DB プール /
    // StreamHub / AtpCommitService を共有し、ジョブで起きたイベントも WebSocket へ届く）
    let worker_inbox = build_inbox_context(
        &pool,
        &local_domain,
        secrets.ap_private_key_pem.clone(),
        Arc::clone(&api_state.stream_hub),
        Arc::clone(&job_queue),
        Arc::clone(&api_state.atp_service),
    );
    let worker = tokio::spawn(seiran_federation_worker::run(
        WorkerConfig {
            queue: job_queue,
            pool,
            ap_client: Arc::clone(&api_state.ap_client),
            delivery: delivery_config(&local_domain, &secrets),
            inbox: Some(worker_inbox),
            follow_exec: Some(api_state.follow_exec_config()),
            atp_service: Arc::clone(&api_state.atp_service),
            encryption_key: secrets.encryption_key_bytes(),
        },
        shutdown.clone(),
    ));

    // パスが衝突しないため単一ポートに合流できる
    let app = seiran_api::router(api_state)
        .merge(seiran_federation_inbox::router(inbox_state))
        // サードパーティクライアント（Misskey互換クライアント等）がどのパスを叩いて
        // 404/401になっているか事後から追えるようにする（アクセスログが他に無いため）。
        // クエリ文字列は記録しない（`/api/streaming?token=...`等、JWTがそのまま
        // ログに残るのを避けるため、パスのみを使う）。
        .layer(
            tower_http::trace::TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    tracing::info_span!(
                        "request",
                        method = %request.method(),
                        path = %request.uri().path(),
                    )
                })
                .on_response(
                    tower_http::trace::DefaultOnResponse::new().level(tracing::Level::INFO),
                ),
        );
    serve(app, env_port("PORT", 3000), shutdown).await?;
    let _ = tokio::join!(worker, firehose);
    Ok(())
}

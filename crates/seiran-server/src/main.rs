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
//! | `worker` | 非同期ジョブ実行エンジン（DB 不要） | なし |
//! | `firehose` | Bluesky Firehose リスナー | なし |
//!
//! 大規模サーバーでは同じイメージを `--role` 違いで複数コンテナ起動し、
//! ワーカー負荷分散などのスケールアウトを行う。

use std::sync::Arc;
use std::time::Duration;

use seiran_common::atp::{AtpCommitEvent, AtpCommitService};
use seiran_common::repository::{
    InstanceDomainRepository, PgActorRepository, PgBlockRepository, PgFollowRepository,
    PgHashtagRepository, PgInstanceDomainRepository, PgListRepository, PgNotificationRepository,
    PgPostRepository, PgReactionRepository, PgRemoteEmojiRepository,
};
use seiran_common::{
    ap::ApClient, create_job_queue, db::recommended_max_connections, get_db_pool,
    resolve_local_domain, run_migrations, DeliveryConfig, FollowExecConfig, InboxContext, JobQueue,
    LocalDomain, Secrets, SecretsFile, StreamHub, DEFAULT_MAX_CONCURRENT_JOBS,
};
use sqlx::PgPool;
use tokio::sync::broadcast;

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

async fn serve(app: axum::Router, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("[seiran-server] リッスン開始: http://{}", addr);
    axum::serve(listener, app).await?;
    Ok(())
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

    if role == Role::Worker {
        return run_standalone_worker().await;
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
    // `all` ロールは常に InMemory（同一プロセス内で api/federation/worker が動くため
    // 外部ミドルウェア不要）。split-role（api/federation 単独起動）は REDIS_URL の
    // 有無で Redis/InMemory を切り替える（create_job_queue のロジック参照）。
    let job_queue = create_job_queue(role == Role::All).await;
    // ATP コミットイベントの Redis プロセス間配信ブリッジも同じ方針: `all` は常に無効
    // （`api` role を複数レプリカで水平スケールする場合のみ必要。単一プロセス内なら
    // event_tx の直接配信で十分）。split-role の api ロールでは REDIS_URL があれば有効化する。
    let atp_event_redis_url = if role == Role::All {
        None
    } else {
        std::env::var("REDIS_URL").ok().filter(|s| !s.is_empty())
    };
    // Jetstream接続の排他制御（複数インスタンス起動時のリーダー選出）専用のRedis URL。
    // `atp_event_redis_url`と違い、`all`ロールでも複数起動（無停止バージョンアップ中の
    // 一時的なスケールアウト等）を検知したいため、ロールに関わらずそのまま読む
    // （Doc3 §14.2、Doc6既知の課題）。
    let jetstream_redis_url = std::env::var("REDIS_URL").ok().filter(|s| !s.is_empty());

    let shared = SharedResources {
        pool,
        secrets,
        http_client,
        local_domain,
        job_queue,
        atp_event_redis_url,
        jetstream_redis_url,
    };
    match role {
        Role::Firehose => {
            // スタンドアロン firehose は WebSocket 配信先がないため空の StreamHub を使用
            seiran_atp_repo::run(
                shared.pool,
                shared.http_client,
                Arc::new(StreamHub::new()),
                shared.jetstream_redis_url,
                false,
                shared.job_queue,
            )
            .await;
        }
        Role::Api => {
            let state = seiran_api::init_state(
                shared.pool,
                shared.secrets,
                shared.http_client,
                shared.local_domain,
                shared.job_queue,
                shared.atp_event_redis_url,
            )
            .await;
            seiran_api::spawn_startup_tasks(&state);
            seiran_api::spawn_gc_tasks(&state);
            serve(seiran_api::router(state), env_port("PORT", 3000)).await?;
        }
        Role::Federation => {
            // 単独 federation ロールでは WS 購読者（api）が居ないため新規ハブで可。
            let state = seiran_federation_inbox::init_state(
                shared.pool,
                &shared.secrets,
                shared.http_client,
                shared.local_domain,
                Arc::new(StreamHub::new()),
                shared.job_queue,
            );
            serve(
                seiran_federation_inbox::router(state),
                env_port("FEDERATION_INBOX_PORT", 3001),
            )
            .await?;
        }
        Role::All => run_all(shared).await?,
        Role::Worker => unreachable!("worker は先頭で分岐済み"),
    }

    Ok(())
}

/// worker 以外のロールで共有するプロセス内リソース。
struct SharedResources {
    pool: PgPool,
    secrets: Arc<Secrets>,
    http_client: Arc<reqwest::Client>,
    local_domain: LocalDomain,
    job_queue: Arc<dyn JobQueue>,
    atp_event_redis_url: Option<String>,
    jetstream_redis_url: Option<String>,
}

/// standalone worker ロール。
///
/// worker も BskyVideoPoll 等 DB アクセスが必要なジョブを扱うため、単独起動時も
/// DB に接続する。
/// AP 配送ジョブ（ApDelivery）が署名に AP 鍵を使うため、シークレットも読み込む。
async fn run_standalone_worker() -> Result<(), Box<dyn std::error::Error>> {
    let secrets = SecretsFile::from_env().load_or_create()?;
    tracing::info!("[seiran-server] シークレット読み込み完了");
    // standalone worker はHTTPを持たず、WorkerEngineの同時実行数分だけDBを使う。
    let pool = connect_db(DEFAULT_MAX_CONCURRENT_JOBS as u32).await?;
    let http_client = build_http_client()?;
    let worker_local_domain = resolve_local_domain_from_env(&pool).await;
    let delivery = delivery_config(&worker_local_domain, &secrets);
    // split-role（standalone worker）: REDIS_URL があれば api/federation プロセスと
    // キューを共有できる。未設定なら自分専用の InMemory になる既知の制約（create_job_queue 参照）。
    let queue = create_job_queue(false).await;
    // standalone worker には WS 購読者もATPコミットイベントの他プロセス購読者も
    // 居ないため、ここ専用の使い捨て event チャンネルで良い（`AtpCommitService`
    // 自体はDBへのコミット・`atp_repo_events`記録は行う。リアルタイム配信不要な
    // フォローインポートの用途では実害がない、`account_withdraw_unfollow_all` と同じ判断）。
    let (follow_exec_atp_tx, _) = broadcast::channel::<AtpCommitEvent>(16);
    let worker_atp_service = Arc::new(AtpCommitService::new(
        pool.clone(),
        Arc::new(follow_exec_atp_tx),
        Arc::clone(&http_client),
        worker_local_domain.clone(),
    ));
    // standalone worker には WS 接続クライアントが居ないため空の StreamHub を使う
    // （InboundActivityProcess の realtime 配信は no-op になる。Role::Firehose と同じ扱い）。
    let inbox = build_inbox_context(
        &pool,
        &worker_local_domain,
        secrets.ap_private_key_pem.clone(),
        Arc::new(StreamHub::new()),
        Arc::clone(&queue),
        Arc::clone(&worker_atp_service),
    );
    let follow_exec = build_follow_exec_config(
        &pool,
        &worker_local_domain,
        secrets.ap_private_key_pem.clone(),
        Arc::new(StreamHub::new()),
        worker_atp_service,
    );
    seiran_federation_worker::run(
        queue,
        pool,
        Arc::new(ApClient::new(http_client)),
        delivery,
        Some(inbox),
        Some(follow_exec),
        secrets.encryption_key_bytes(),
    )
    .await;
    Ok(())
}

/// `all` ロール: api + federation を1ポートに合流し、firehose と worker を同一プロセスで起動する。
async fn run_all(shared: SharedResources) -> Result<(), Box<dyn std::error::Error>> {
    let SharedResources {
        pool,
        secrets,
        http_client,
        local_domain,
        job_queue,
        atp_event_redis_url,
        jetstream_redis_url,
    } = shared;

    // api ロール
    let api_state = seiran_api::init_state(
        pool.clone(),
        Arc::clone(&secrets),
        Arc::clone(&http_client),
        local_domain.clone(),
        Arc::clone(&job_queue),
        atp_event_redis_url,
    )
    .await;
    seiran_api::spawn_startup_tasks(&api_state);
    seiran_api::spawn_gc_tasks(&api_state);

    // federation ロール（#37: ストリーミングハブを api と共有して跨いで配信。
    // job_queue も api/worker と同一インスタンスを共有する）
    let inbox_state = seiran_federation_inbox::init_state(
        pool.clone(),
        &secrets,
        Arc::clone(&http_client),
        local_domain.clone(),
        Arc::clone(&api_state.stream_hub),
        Arc::clone(&job_queue),
    );

    // firehose リスナーをバックグラウンド起動（stream_hub を共有して WebSocket 配信）
    {
        let pool = pool.clone();
        let http = Arc::clone(&http_client);
        let hub = Arc::clone(&api_state.stream_hub);
        let queue = Arc::clone(&job_queue);
        tokio::spawn(async move {
            seiran_atp_repo::run(pool, http, hub, jetstream_redis_url, true, queue).await
        });
    }

    // worker をバックグラウンド起動（api ロールと同じ ApClient / JobQueue / DB プールを共有）
    let worker_ap_client = Arc::clone(&api_state.ap_client);
    let worker_delivery = delivery_config(&local_domain, &secrets);
    // InboundActivityProcess 用: api ロールと同じ stream_hub を共有するため、
    // 埋め込み worker で処理したインバウンド活動のリアルタイム通知も api の
    // WebSocket クライアントへ届く。
    let worker_inbox = build_inbox_context(
        &pool,
        &local_domain,
        secrets.ap_private_key_pem.clone(),
        Arc::clone(&api_state.stream_hub),
        Arc::clone(&job_queue),
        Arc::clone(&api_state.atp_service),
    );
    // api ロールと同じリポジトリ・AtpCommitService・StreamHub を共有するため、
    // フォローインポートで成立したフォローの通知もリアルタイムに配信される。
    let worker_follow_exec = api_state.follow_exec_config();
    let worker_encryption_key = secrets.encryption_key_bytes();
    tokio::spawn(async move {
        seiran_federation_worker::run(
            job_queue,
            pool,
            worker_ap_client,
            worker_delivery,
            Some(worker_inbox),
            Some(worker_follow_exec),
            worker_encryption_key,
        )
        .await
    });

    // パスが衝突しないため単一ポートに合流できる
    let app = seiran_api::router(api_state).merge(seiran_federation_inbox::router(inbox_state));
    serve(app, env_port("PORT", 3000)).await
}

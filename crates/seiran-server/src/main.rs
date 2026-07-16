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

use seiran_common::repository::{
    PgActorRepository, PgFollowRepository, PgNotificationRepository, PgPostRepository, PgReactionRepository,
};
use seiran_common::{
    ap::ApClient, create_job_queue, get_db_pool, run_migrations, DeliveryConfig, InboxContext,
    SecretsFile, StreamHub,
};
use sqlx::PgPool;

/// `InboxContext`（InboundActivityProcess ジョブ用）を組み立てる。
/// standalone worker と `all` ロール埋め込み worker の両方から呼ばれる共通ヘルパー。
fn build_inbox_context(
    pool: &PgPool,
    local_domain: &str,
    ap_private_key_pem: Option<String>,
    stream_hub: Arc<StreamHub>,
) -> InboxContext {
    InboxContext {
        actor_repo: Arc::new(PgActorRepository::new(pool.clone())),
        follow_repo: Arc::new(PgFollowRepository::new(pool.clone())),
        post_repo: Arc::new(PgPostRepository::new(pool.clone())),
        reaction_repo: Arc::new(PgReactionRepository::new(pool.clone())),
        notification_repo: Arc::new(PgNotificationRepository::new(pool.clone())),
        local_domain: local_domain.to_string(),
        ap_private_key_pem: ap_private_key_pem.unwrap_or_default(),
        stream_hub,
    }
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
                tracing::warn!("[seiran-server] 不明なロール '{}' → 'all' で起動します", other);
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    let role = Role::resolve();
    tracing::info!("[seiran-server] ロール: {:?}", role);

    // worker も BskyVideoPoll 等 DB アクセスが必要なジョブを扱うため、単独起動時も
    // DB に接続する（以前は「DB不要」だったが、ジョブハンドラの実装が進んだため変更）。
    // AP 配送ジョブ（ApDelivery）が署名に AP 鍵を使うため、シークレットも読み込む。
    if role == Role::Worker {
        let secrets = SecretsFile::from_env().load_or_create()?;
        tracing::info!("[seiran-server] シークレット読み込み完了");
        let pool = get_db_pool().await?;
        tracing::info!("[seiran-server] DB 接続完了");
        let http_client = Arc::new(
            reqwest::Client::builder()
                .user_agent("seiran-federation/0.1.0")
                .build()?,
        );
        let worker_local_domain =
            std::env::var("LOCAL_DOMAIN").unwrap_or_else(|_| "localhost".to_string());
        let delivery = DeliveryConfig {
            local_domain: worker_local_domain.clone(),
            ap_private_key_pem: secrets.ap_private_key_pem.clone(),
            ap_public_key_pem: secrets.ap_public_key_pem.clone(),
        };
        // standalone worker には WS 接続クライアントが居ないため空の StreamHub を使う
        // （InboundActivityProcess の realtime 配信は no-op になる。Role::Firehose と同じ扱い）。
        let inbox = build_inbox_context(
            &pool, &worker_local_domain, secrets.ap_private_key_pem.clone(), Arc::new(StreamHub::new()),
        );
        // split-role（standalone worker）: REDIS_URL があれば api/federation プロセスと
        // キューを共有できる。未設定なら自分専用の InMemory になる既知の制約（create_job_queue 参照）。
        let queue = create_job_queue(false).await;
        seiran_federation_worker::run(queue, pool, Arc::new(ApClient::new(http_client)), delivery, Some(inbox)).await;
        return Ok(());
    }

    // ── 共有リソース（プロセス内で一度だけ生成し各ロールへ渡す）──
    let secrets = Arc::new(SecretsFile::from_env().load_or_create()?);
    tracing::info!("[seiran-server] シークレット読み込み完了");

    let pool = get_db_pool().await?;
    tracing::info!("[seiran-server] DB 接続完了");

    let http_client = Arc::new(
        reqwest::Client::builder()
            .user_agent("seiran-federation/0.1.0")
            .build()?,
    );
    let local_domain = std::env::var("LOCAL_DOMAIN").unwrap_or_else(|_| "localhost".to_string());
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

    match role {
        Role::Firehose => {
            // スタンドアロン firehose は WebSocket 配信先がないため空の StreamHub を使用
            let hub = Arc::new(StreamHub::new());
            seiran_atp_repo::run(pool, http_client, hub, jetstream_redis_url, false).await;
        }

        Role::Api => {
            run_migrations(&pool).await?;
            tracing::info!("[seiran-server] マイグレーション適用完了");

            let state = seiran_api::init_state(
                pool, secrets, http_client, local_domain, job_queue, atp_event_redis_url,
            )
            .await;
            seiran_api::spawn_startup_tasks(&state);
            seiran_api::spawn_gc_tasks(&state);
            serve(seiran_api::router(state), env_port("PORT", 3000)).await?;
        }

        Role::Federation => {
            // 単独 federation ロールでは WS 購読者（api）が居ないため新規ハブで可。
            let state = seiran_federation_inbox::init_state(
                pool,
                &secrets,
                http_client,
                local_domain,
                Arc::new(StreamHub::new()),
                job_queue,
            );
            serve(
                seiran_federation_inbox::router(state),
                env_port("FEDERATION_INBOX_PORT", 3001),
            )
            .await?;
        }

        Role::All => {
            run_migrations(&pool).await?;
            tracing::info!("[seiran-server] マイグレーション適用完了");

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
            let shared_hub = Arc::clone(&api_state.stream_hub);
            let inbox_state = seiran_federation_inbox::init_state(
                pool.clone(),
                &secrets,
                Arc::clone(&http_client),
                local_domain.clone(),
                shared_hub,
                Arc::clone(&job_queue),
            );

            // firehose リスナーをバックグラウンド起動（stream_hub を共有して WebSocket 配信）
            {
                let pool = pool.clone();
                let http = Arc::clone(&http_client);
                let hub = Arc::clone(&api_state.stream_hub);
                let redis_url = jetstream_redis_url.clone();
                tokio::spawn(async move { seiran_atp_repo::run(pool, http, hub, redis_url, true).await });
            }

            // worker をバックグラウンド起動（api ロールと同じ ApClient / JobQueue / DB プールを共有）
            let worker_ap_client = Arc::clone(&api_state.ap_client);
            let worker_queue = Arc::clone(&job_queue);
            let worker_pool = pool.clone();
            let worker_delivery = DeliveryConfig {
                local_domain: local_domain.clone(),
                ap_private_key_pem: secrets.ap_private_key_pem.clone(),
                ap_public_key_pem: secrets.ap_public_key_pem.clone(),
            };
            // InboundActivityProcess 用: api ロールと同じ stream_hub を共有するため、
            // 埋め込み worker で処理したインバウンド活動のリアルタイム通知も api の
            // WebSocket クライアントへ届く。
            let worker_inbox = build_inbox_context(
                &pool, &local_domain, secrets.ap_private_key_pem.clone(), Arc::clone(&api_state.stream_hub),
            );
            tokio::spawn(async move {
                seiran_federation_worker::run(
                    worker_queue, worker_pool, worker_ap_client, worker_delivery, Some(worker_inbox),
                )
                .await
            });

            // パスが衝突しないため単一ポートに合流できる
            let app =
                seiran_api::router(api_state).merge(seiran_federation_inbox::router(inbox_state));

            serve(app, env_port("PORT", 3000)).await?;
        }

        Role::Worker => unreachable!("worker は先頭で分岐済み"),
    }

    Ok(())
}

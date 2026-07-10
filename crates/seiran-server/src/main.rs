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

use seiran_common::{ap::ApClient, get_db_pool, run_migrations, SecretsFile, StreamHub};

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
                eprintln!("[seiran-server] 不明なロール '{}' → 'all' で起動します", other);
                Role::All
            }
        }
    }
}

async fn serve(app: axum::Router, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("[seiran-server] リッスン開始: http://{}", addr);
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
    // blurhash 0.2.x のオフバイワンバグによる既知パニックを stderr に出力しない。
    // catch_unwind で回復済みのため、ログノイズを抑制するだけで動作は正常。
    std::panic::set_hook(Box::new(|info| {
        let msg = info.to_string();
        if !msg.contains("blurhash") {
            eprintln!("{}", msg);
        }
    }));

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let _ = dotenvy::dotenv();

    let role = Role::resolve();
    eprintln!("[seiran-server] ロール: {:?}", role);

    // worker は現状 DB を必要としない（InMemoryJobQueue のスタブ）ため先に分岐する。
    // 単独プロセスのため、この場合のみここで HTTP クライアントを新規生成する
    // （`all` ロールでは下で作る共有 http_client を再利用する。二重生成しない）。
    if role == Role::Worker {
        let http_client = Arc::new(
            reqwest::Client::builder()
                .user_agent("seiran-federation/0.1.0")
                .build()?,
        );
        seiran_federation_worker::run(Arc::new(ApClient::new(http_client))).await;
        return Ok(());
    }

    // ── 共有リソース（プロセス内で一度だけ生成し各ロールへ渡す）──
    let secrets = Arc::new(SecretsFile::from_env().load_or_create()?);
    eprintln!("[seiran-server] シークレット読み込み完了");

    let pool = get_db_pool().await?;
    eprintln!("[seiran-server] DB 接続完了");

    let http_client = Arc::new(
        reqwest::Client::builder()
            .user_agent("seiran-federation/0.1.0")
            .build()?,
    );
    let local_domain = std::env::var("LOCAL_DOMAIN").unwrap_or_else(|_| "localhost".to_string());

    match role {
        Role::Firehose => {
            // スタンドアロン firehose は WebSocket 配信先がないため空の StreamHub を使用
            let hub = Arc::new(StreamHub::new());
            seiran_atp_repo::run(pool, http_client, hub).await;
        }

        Role::Api => {
            run_migrations(&pool).await?;
            eprintln!("[seiran-server] マイグレーション適用完了");

            let state =
                seiran_api::init_state(pool, secrets, http_client, local_domain).await;
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
            );
            serve(
                seiran_federation_inbox::router(state),
                env_port("FEDERATION_INBOX_PORT", 3001),
            )
            .await?;
        }

        Role::All => {
            run_migrations(&pool).await?;
            eprintln!("[seiran-server] マイグレーション適用完了");

            // api ロール
            let api_state = seiran_api::init_state(
                pool.clone(),
                Arc::clone(&secrets),
                Arc::clone(&http_client),
                local_domain.clone(),
            )
            .await;
            seiran_api::spawn_startup_tasks(&api_state);
            seiran_api::spawn_gc_tasks(&api_state);

            // federation ロール（#37: ストリーミングハブを api と共有して跨いで配信）
            let shared_hub = Arc::clone(&api_state.stream_hub);
            let inbox_state = seiran_federation_inbox::init_state(
                pool.clone(),
                &secrets,
                Arc::clone(&http_client),
                local_domain.clone(),
                shared_hub,
            );

            // firehose リスナーをバックグラウンド起動（stream_hub を共有して WebSocket 配信）
            {
                let pool = pool.clone();
                let http = Arc::clone(&http_client);
                let hub = Arc::clone(&api_state.stream_hub);
                tokio::spawn(async move { seiran_atp_repo::run(pool, http, hub).await });
            }

            // worker をバックグラウンド起動（api ロールと同じ ApClient / コネクションプールを共有）
            let worker_ap_client = Arc::clone(&api_state.ap_client);
            tokio::spawn(async move { seiran_federation_worker::run(worker_ap_client).await });

            // パスが衝突しないため単一ポートに合流できる
            let app =
                seiran_api::router(api_state).merge(seiran_federation_inbox::router(inbox_state));

            serve(app, env_port("PORT", 3000)).await?;
        }

        Role::Worker => unreachable!("worker は先頭で分岐済み"),
    }

    Ok(())
}

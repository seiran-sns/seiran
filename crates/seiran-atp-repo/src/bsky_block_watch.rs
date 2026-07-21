//! `app.bsky.graph.block` の無絞り込みJetstream監視。
//!
//! Bluesky公式APIには「自分をブロックしている人一覧」を返すエンドポイントが無い
//! （プライバシー保護のため意図的に非公開）ため、ポーリングでは相手発ブロックを検知できない。
//! 代わりに Jetstream で `app.bsky.graph.block` レコードのコミットをリアルタイム購読し、
//! `subject`（ブロックされた側のDID）がローカルユーザーのものと一致するイベントだけを拾う。
//!
//! `firehose.rs`（`app.bsky.feed.post`/`app.bsky.feed.like`、`wantedDids`で絞り込み）とは
//! 別の独立したJetstream接続にする。ブロックする側は seiran ユーザーをフォローしているとは
//! 限らない（見ず知らずの相手からもブロックされうる）ため、`wantedDids`による絞り込みは
//! 使えず、コレクション全体を無絞り込みで受信する必要がある。実測ではこのコレクションの
//! イベントは全世界で約2件/秒程度（1日あたり約18万件）であり、無絞り込みでも現実的な負荷。

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::StreamExt;

use seiran_common::jetstream_leader::{self, JetstreamLeaderElector};
use seiran_common::repository::{ActorRepository, BlockRepository, PgActorRepository, PgBlockRepository};

use crate::firehose::resolve_or_upsert_bsky_actor;

const JETSTREAM_BLOCK_URL: &str =
    "wss://jetstream1.us-east.bsky.network/subscribe?wantedCollections=app.bsky.graph.block";

/// post/like用（`jetstream_leader::DEFAULT_LEADER_KEY`）とは独立にリーダーを選出するための
/// 専用リースキー。無絞り込み接続はコスト・特性が異なるため、post/like側の接続とは
/// 別インスタンスがリーダーになってもよい設計にしている。
const BLOCK_WATCH_LEADER_KEY: &str = "seiran:jetstream:leader:block_watch";

/// cursor永続化キー（`site_settings`）。`firehose.rs`の`jetstream_cursor`と衝突しないよう
/// 別キーにする。
const BLOCK_CURSOR_KEY: &str = "jetstream_block_cursor";
const CURSOR_SAVE_INTERVAL: Duration = Duration::from_secs(5);

/// リーダー選出に応じて、無絞り込みJetstream接続の起動・停止を切り替える。
/// `firehose.rs::run`と同じ制御パターン（`docs/protocols.md` 10節）。
pub async fn run(pool: PgPool, http: Arc<reqwest::Client>, redis_url: Option<String>, is_monolith: bool) {
    let mut elector: Option<JetstreamLeaderElector> = None;
    let mut current_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut poll = tokio::time::interval(jetstream_leader::LEASE_CHECK_INTERVAL);

    loop {
        poll.tick().await;

        let should_run = match &redis_url {
            None => is_monolith,
            Some(url) => {
                if elector.is_none() {
                    match JetstreamLeaderElector::connect(url, BLOCK_WATCH_LEADER_KEY).await {
                        Ok(e) => elector = Some(e),
                        Err(e) => tracing::error!("[Jetstream/BlockWatch] Redis接続失敗: {}", e),
                    }
                }
                match &elector {
                    Some(e) => match e.try_acquire_or_renew().await {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::error!("[Jetstream/BlockWatch] Redisリース確認失敗: {}。再接続を試みます。", e);
                            elector = None;
                            is_monolith
                        }
                    },
                    None => is_monolith,
                }
            }
        };

        match (should_run, current_task.is_some()) {
            (true, false) => {
                tracing::info!("[Jetstream/BlockWatch] リーダーに昇格（またはRedis未使用の単独運用）。接続開始。");
                let pool = pool.clone();
                let http = Arc::clone(&http);
                current_task = Some(tokio::spawn(run_loop(pool, http)));
            }
            (false, true) => {
                tracing::info!("[Jetstream/BlockWatch] リーダーでなくなったため切断。");
                if let Some(task) = current_task.take() {
                    task.abort();
                }
            }
            _ => {}
        }
    }
}

async fn run_loop(pool: PgPool, http: Arc<reqwest::Client>) {
    let mut backoff_secs = 2u64;
    loop {
        match connect_and_process(&pool, &http).await {
            Ok(()) => {
                tracing::info!("[Jetstream/BlockWatch] 接続終了（正常）。再接続します。");
                backoff_secs = 2;
            }
            Err(e) => {
                tracing::error!("[Jetstream/BlockWatch] エラー: {}。{}秒後に再接続します。", e, backoff_secs);
                sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(120);
            }
        }
    }
}

async fn load_cursor(pool: &PgPool) -> Option<i64> {
    sqlx::query_scalar::<_, String>("SELECT value FROM site_settings WHERE key = $1")
        .bind(BLOCK_CURSOR_KEY)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
}

async fn save_cursor(pool: &PgPool, time_us: i64) {
    if let Err(e) = sqlx::query(
        "INSERT INTO site_settings (key, value, updated_at) VALUES ($1, $2, NOW())
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()",
    )
    .bind(BLOCK_CURSOR_KEY)
    .bind(time_us.to_string())
    .execute(pool)
    .await
    {
        tracing::error!("[Jetstream/BlockWatch] cursor保存失敗: {}", e);
    }
}

#[derive(Deserialize)]
struct JetstreamTimeUs {
    time_us: i64,
}

#[derive(Deserialize)]
struct JetstreamEvent {
    did: String,
    kind: String,
    commit: Option<JetstreamCommit>,
}

#[derive(Deserialize)]
struct JetstreamCommit {
    operation: String, // "create" | "delete"（"update"はブロックには存在しない）
    collection: String,
    rkey: String,
    #[serde(default)]
    record: Option<JsonValue>,
}

async fn connect_and_process(pool: &PgPool, http: &Arc<reqwest::Client>) -> Result<(), String> {
    let cursor = load_cursor(pool).await;
    let mut url = JETSTREAM_BLOCK_URL.to_string();
    if let Some(c) = cursor {
        url.push_str(&format!("&cursor={}", c));
    }
    tracing::info!("[Jetstream/BlockWatch] 接続中: {}", url);

    let (mut ws_stream, _) = connect_async(&url)
        .await
        .map_err(|e| format!("WebSocket 接続失敗: {}", e))?;

    tracing::info!("[Jetstream/BlockWatch] 接続成功。イベント受信中...");

    let mut last_saved_at = tokio::time::Instant::now() - CURSOR_SAVE_INTERVAL;

    while let Some(msg) = ws_stream.next().await {
        let msg = msg.map_err(|e| format!("WebSocket 受信エラー: {}", e))?;
        let Message::Text(text) = msg else { continue };

        if let Ok(t) = serde_json::from_str::<JetstreamTimeUs>(&text)
            && last_saved_at.elapsed() >= CURSOR_SAVE_INTERVAL
        {
            save_cursor(pool, t.time_us).await;
            last_saved_at = tokio::time::Instant::now();
        }

        if let Err(e) = process_message(&text, pool, http).await {
            tracing::error!("[Jetstream/BlockWatch] メッセージ処理エラー（スキップ）: {}", e);
        }
    }

    Ok(())
}

async fn process_message(text: &str, pool: &PgPool, http: &Arc<reqwest::Client>) -> Result<(), String> {
    let event: JetstreamEvent =
        serde_json::from_str(text).map_err(|e| format!("JSON パースエラー: {}", e))?;

    if event.kind != "commit" {
        return Ok(());
    }
    let Some(commit) = event.commit else { return Ok(()) };
    if commit.collection != "app.bsky.graph.block" {
        return Ok(());
    }

    let actor_repo = PgActorRepository::new(pool.clone());
    let block_repo = PgBlockRepository::new(pool.clone());

    match commit.operation.as_str() {
        "create" => {
            let Some(record) = commit.record else { return Ok(()) };
            let Some(subject_did) = record.get("subject").and_then(|v| v.as_str()) else {
                return Ok(());
            };

            // subjectがローカルユーザーでなければ無関係のブロック（大半のイベントはここで
            // 早期リターンする）。
            let Some(local_actor) = actor_repo
                .find_by_did(subject_did)
                .await
                .map_err(|e| format!("DB検索失敗: {}", e))?
            else {
                return Ok(());
            };
            if local_actor.actor_type != "local" {
                return Ok(());
            }

            let blocker_actor_id = resolve_or_upsert_bsky_actor(pool, http, &event.did)
                .await
                .map_err(|e| format!("blocker アクター解決失敗: {}", e))?;

            block_repo
                .insert(blocker_actor_id, local_actor.id, Some(&commit.rkey))
                .await
                .map_err(|e| format!("blocks INSERT 失敗: {}", e))?;

            tracing::info!(
                "[Jetstream/BlockWatch] {} が '{}' をブロック（記録完了）",
                event.did, local_actor.username
            );
        }
        "delete" => {
            // Jetstreamのdeleteイベントはレコード本体（subject）を伴わないため、
            // did（blocker）+ rkeyの組でしか該当行を特定できない。
            let Some(blocker) = actor_repo
                .find_by_did(&event.did)
                .await
                .map_err(|e| format!("DB検索失敗: {}", e))?
            else {
                return Ok(()); // 未知のDID（ローカルユーザーをブロックしたことが無い）
            };

            block_repo
                .delete_by_blocker_and_rkey(blocker.id, &commit.rkey)
                .await
                .map_err(|e| format!("blocks DELETE 失敗: {}", e))?;

            tracing::info!("[Jetstream/BlockWatch] {} のブロック解除を検知（rkey={}）", event.did, commit.rkey);
        }
        _ => {}
    }

    Ok(())
}

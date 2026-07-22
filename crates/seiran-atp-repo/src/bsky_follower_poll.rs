//! Bsky側フォロワー検知ポーリング。
//!
//! Jetstream の `wantedDids` は「発行者（投稿者）DID」でのフィルタであり、フォロー元
//! （＝新規に自分をフォローしてきたアクター）を事前に知る手段が無いため、購読対象に
//! 含めることができない。そのため `app.bsky.graph.getFollowers`（AppView 公開エンドポイント）
//! をローカル Bsky リンク済みユーザーごとに定期ポーリングし、フォロワー一覧の差分から
//! 新規フォローを検知する。
//!
//! 機能導入時点で既に実フォロー済みの全フォロワーが「新規フォロー」として一斉検出されて
//! 通知が大量発生するのを防ぐため、アクター単位で `actors.bsky_followers_baseline_done_at`
//! （NULL=未シード）を見て、初回ポーリングは「フォロワー一覧を取り込むだけで通知は出さない」
//! baseline seed として扱う。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use seiran_common::atp::fetch_bsky_followers;
use seiran_common::generate_snowflake_id;
use seiran_common::repository::{
    ActorRepository, FollowRepository, NotificationKind, NotificationRepository,
    PgActorRepository, PgFollowRepository, PgNotificationRepository,
};
use seiran_common::streaming::StreamHub;
use sqlx::{PgPool, Row};

/// 1ページあたりの取得件数（`getFollowers` の `limit`）。
const PAGE_LIMIT: u32 = 100;
/// baseline 済みユーザーの定常ポーリング時、既知フォロワーに到達しなかった場合の安全上限。
const STEADY_STATE_MAX_PAGES: u32 = 20;
/// 未 baseline ユーザーの初回シード時、全フォロワーを辿り切るための安全上限。
const HARD_MAX_PAGES: u32 = 1000;

fn poll_interval() -> Duration {
    let secs = std::env::var("BSKY_FOLLOWER_POLL_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(60);
    Duration::from_secs(secs)
}

/// フォロワー検知ポーリングを常駐実行する。
pub async fn run(pool: PgPool, http: Arc<reqwest::Client>, stream_hub: Arc<StreamHub>) {
    let mut interval = tokio::time::interval(poll_interval());
    loop {
        interval.tick().await;
        poll_once(&pool, &http, &stream_hub).await;
    }
}

async fn poll_once(pool: &PgPool, http: &reqwest::Client, stream_hub: &StreamHub) {
    let users = match sqlx::query(
        "SELECT id, at_did FROM actors
         WHERE actor_type = 'local' AND at_did IS NOT NULL AND at_signing_key_pem IS NOT NULL
           AND withdrawn_at IS NULL",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[BskyFollowerPoll] 対象ユーザー取得失敗: {}", e);
            return;
        }
    };

    for row in users {
        let actor_id: i64 = match row.try_get("id") {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("[BskyFollowerPoll] id 取得失敗: {}", e);
                continue;
            }
        };
        let did: String = match row.try_get("at_did") {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("[BskyFollowerPoll] at_did 取得失敗 actor_id={}: {}", actor_id, e);
                continue;
            }
        };
        if let Err(e) = poll_user(pool, http, stream_hub, actor_id, &did).await {
            tracing::warn!("[BskyFollowerPoll] actor_id={} のポーリング失敗: {}", actor_id, e);
        }
    }
}

async fn poll_user(
    pool: &PgPool,
    http: &reqwest::Client,
    stream_hub: &StreamHub,
    local_actor_id: i64,
    did: &str,
) -> Result<(), String> {
    let baseline_done: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT bsky_followers_baseline_done_at FROM actors WHERE id = $1")
            .bind(local_actor_id)
            .fetch_one(pool)
            .await
            .map_err(|e| format!("baseline取得失敗: {}", e))?;
    let is_baseline_done = baseline_done.is_some();

    let known_follower_ids: HashSet<i64> = sqlx::query_scalar(
        "SELECT follower_actor_id FROM follows WHERE target_actor_id = $1",
    )
    .bind(local_actor_id)
    .fetch_all(pool)
    .await
    .map_err(|e| format!("既存フォロワー取得失敗: {}", e))?
    .into_iter()
    .collect();

    let actor_repo = PgActorRepository::new(pool.clone());
    let follow_repo = PgFollowRepository::new(pool.clone());
    let notification_repo = PgNotificationRepository::new(pool.clone());

    let max_pages = if is_baseline_done { STEADY_STATE_MAX_PAGES } else { HARD_MAX_PAGES };
    let mut cursor: Option<String> = None;

    'paging: for _ in 0..max_pages {
        let (followers, next_cursor) =
            fetch_bsky_followers(http, did, cursor.as_deref(), PAGE_LIMIT).await?;
        if followers.is_empty() {
            break;
        }

        for f in &followers {
            let follower_actor_id = match actor_repo.find_by_did(&f.did).await {
                Ok(Some(existing)) => existing.id,
                Ok(None) => {
                    let new_id = generate_snowflake_id(Utc::now());
                    actor_repo
                        .upsert_remote_bsky(
                            new_id,
                            &f.did,
                            &f.handle,
                            f.display_name.as_deref(),
                            f.avatar.as_deref(),
                            Utc::now(),
                        )
                        .await
                        .map_err(|e| format!("upsert_remote_bsky 失敗 did={}: {}", f.did, e))?
                }
                Err(e) => {
                    tracing::warn!("[BskyFollowerPoll] find_by_did 失敗 did={}: {}", f.did, e);
                    continue;
                }
            };

            if known_follower_ids.contains(&follower_actor_id) {
                if is_baseline_done {
                    // baseline 済み: 新しい順で返る前提のため、既知フォロワーに到達したら
                    // このユーザーについてはこれ以上新規フォローが無いとみなして早期終了する。
                    break 'paging;
                }
                // 未baseline時は最後まで辿ってフォロワー一覧を確定させる（既知でも読み飛ばすだけ）。
                continue;
            }

            follow_repo
                .insert_accepted(follower_actor_id, local_actor_id)
                .await
                .map_err(|e| format!("follows INSERT失敗 follower={}: {}", follower_actor_id, e))?;

            if is_baseline_done {
                stream_hub.publish_event(
                    HashSet::from([local_actor_id]),
                    "follow",
                    serde_json::json!({
                        "actor": { "username": f.handle, "domain": "", "displayName": f.display_name },
                    }),
                );
                let notif_id = generate_snowflake_id(Utc::now());
                let source_uri = format!("bsky-follow:{}:{}", follower_actor_id, local_actor_id);
                if let Err(e) = notification_repo
                    .insert(
                        notif_id,
                        local_actor_id,
                        NotificationKind::Follow,
                        Some(follower_actor_id),
                        None,
                        None,
                        None,
                        Some(&source_uri),
                        None,
                    )
                    .await
                {
                    tracing::error!("[BskyFollowerPoll] notifications INSERT失敗: {}", e);
                }
            }
        }

        if next_cursor.is_none() {
            break;
        }
        cursor = next_cursor;
    }

    if !is_baseline_done {
        sqlx::query("UPDATE actors SET bsky_followers_baseline_done_at = NOW() WHERE id = $1")
            .bind(local_actor_id)
            .execute(pool)
            .await
            .map_err(|e| format!("baseline更新失敗: {}", e))?;
    }

    Ok(())
}

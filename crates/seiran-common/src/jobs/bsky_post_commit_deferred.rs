//! ⑦ 動画添付投稿の Bsky コミット遅延キュー (`bsky_post_commit_deferred`)
//!
//! 動画添付を含む投稿を作成した直後（`app.bsky.video.uploadVideo` への提出はしたが
//! トランスコード未完了）に ATP コミットしてしまうと、`media_files.bsky_video_status`
//! がまだ `ready` になっておらず、`AtpCommitService::commit_post` は常に
//! `app.bsky.embed.external` へフォールバックしてしまう（一度 external でコミットされた
//! 投稿は再コミットされないため、以後 video embed 化されることもない）。
//!
//! このジョブは動画添付を持つ投稿のコミットをここに委譲し、`bsky_video_status` が
//! 確定状態（`ready`/`failed`）になるのを待ってから `commit_post` を呼ぶ。
//! `media_files.created_at` からの経過時間が `SETTLE_TIMEOUT_SECS` を超えたら、
//! 未確定のままでも諦めてコミットする（commit_post 内部の既存フォールバックに委ねる）。
//! 2026-07-17 マイケル指摘・実機再現確認。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::Row;
use tokio::sync::broadcast;

use crate::atp::repo::{BskyPostReply, BskyRefRecord};
use crate::atp::service::AtpCommitService;
use crate::mention::convert_mentions_for_bsky;
use crate::queue::worker::JobContext;

/// `retry_config_for(Job::BskyPostCommitDeferred)` の最大待機時間（60秒）より
/// 少し長く取り、リトライ上限に達する前に時間切れフォールバックが先に効くようにする。
const SETTLE_TIMEOUT_SECS: i64 = 70;

#[allow(clippy::too_many_arguments)]
pub async fn handle(
    actor_id: i64,
    post_id: i64,
    text: String,
    attachment_ids: Vec<i64>,
    reply_root: Option<(String, String)>,
    reply_parent: Option<(String, String)>,
    now: DateTime<Utc>,
    ctx: Arc<JobContext>,
) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        tracing::warn!("[BskyPostCommitDeferred] DB pool 未設定のためスキップ (post_id={})", post_id);
        return Ok(());
    };

    if !attachment_ids.is_empty() {
        let rows = sqlx::query(
            "SELECT bsky_video_status, created_at FROM media_files
             WHERE id = ANY($1) AND mime_type LIKE 'video/%'",
        )
        .bind(&attachment_ids)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("media_files取得失敗: {}", e))?;

        let mut all_settled = true;
        let mut oldest_created_at: Option<DateTime<Utc>> = None;
        for row in &rows {
            let status: Option<String> = row.try_get("bsky_video_status").unwrap_or(None);
            let created_at: DateTime<Utc> = row.try_get("created_at").map_err(|e| format!("created_at取得失敗: {}", e))?;
            if !matches!(status.as_deref(), Some("ready") | Some("failed")) {
                all_settled = false;
            }
            oldest_created_at = Some(oldest_created_at.map_or(created_at, |o: DateTime<Utc>| o.min(created_at)));
        }

        if !all_settled {
            let elapsed_secs = oldest_created_at.map(|c| (Utc::now() - c).num_seconds()).unwrap_or(0);
            if elapsed_secs < SETTLE_TIMEOUT_SECS {
                return Err(format!("動画パイプライン結合待ち（経過{}秒）", elapsed_secs));
            }
            tracing::warn!(
                "[BskyPostCommitDeferred] {}秒経過してもbsky_video_statusが確定しないためフォールバックコミット post_id={}",
                elapsed_secs, post_id
            );
        }
    }

    let cfg = ctx.delivery.as_ref().ok_or_else(|| "配送設定未注入".to_string())?;
    let (bsky_text, bsky_facets) =
        convert_mentions_for_bsky(&text, &cfg.local_domain, pool, ctx.ap_client.http.as_ref()).await;

    let bsky_reply = match (reply_root, reply_parent) {
        (Some((root_uri, root_cid)), Some((parent_uri, parent_cid))) => Some(BskyPostReply {
            root: BskyRefRecord { uri: root_uri, cid: root_cid },
            parent: BskyRefRecord { uri: parent_uri, cid: parent_cid },
        }),
        _ => None,
    };

    // ATPコミット用のサービス。event_txはこのジョブ専用の使い捨てチャンネルで良い
    // （account_withdraw_unfollow_all と同じ理由: subscribeReposのリアルタイム購読者には
    // 届かないが、atp_repo_eventsテーブルへの記録自体は行われるため、他のRelayが
    // 再購読すれば最終的に一貫する）。
    let (event_tx, _rx) = broadcast::channel(16);
    let atp_service = AtpCommitService::new(pool.clone(), Arc::new(event_tx), Arc::clone(&ctx.ap_client.http));

    atp_service
        .commit_post(actor_id, post_id, &bsky_text, bsky_facets, &attachment_ids, now, bsky_reply)
        .await
        .map_err(|e| format!("ATP コミット失敗: {}", e))?;

    tracing::info!("[BskyPostCommitDeferred] コミット完了 post_id={}", post_id);
    Ok(())
}

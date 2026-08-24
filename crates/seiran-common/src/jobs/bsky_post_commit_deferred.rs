//! ⑦ 動画添付投稿の Bsky コミット遅延キュー (`bsky_post_commit_deferred`)
//!
//! Bsky embedとして選択された（#227、明示選択または省略時の固定優先順位、
//! `seiran-api::handlers::notes::delivery::resolve_bsky_embed`）動画/音声添付を含む投稿を
//! 作成した直後（`app.bsky.video.uploadVideo` への提出はしたがトランスコード未完了）に
//! ATP コミットしてしまうと、`media_files.bsky_video_status` がまだ `ready` になっておらず、
//! 常に `app.bsky.embed.external`（視聴ページへのリンクカード）へフォールバックしてしまう
//! （一度 external でコミットされた投稿は再コミットされないため、以後 video embed 化される
//! こともない）。
//!
//! このジョブは選択された添付1件（`pending_media_file_id`）のコミットをここに委譲し、
//! `bsky_video_status` が確定状態（`ready`/`failed`）になるのを待ってから `commit_post` を
//! 呼ぶ。`media_files.created_at` からの経過時間が `SETTLE_TIMEOUT_SECS` を超えたら、
//! 未確定のままでも諦めて視聴ページへのリンクカードでコミットする。
//! 2026-07-17 マイケル指摘・実機再現確認。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::Row;
use tokio::sync::broadcast;

use crate::atp::repo::{BskyEmbed, BskyPostReply, BskyRefRecord};
use crate::atp::service::AtpCommitService;
use crate::mention::convert_mentions_for_bsky;
use crate::queue::worker::JobContext;
use crate::{AUDIO_VIDEO_HEIGHT, AUDIO_VIDEO_WIDTH};

/// `retry_config_for(Job::BskyPostCommitDeferred)` の最大待機時間（60秒）より
/// 少し長く取り、リトライ上限に達する前に時間切れフォールバックが先に効くようにする。
const SETTLE_TIMEOUT_SECS: i64 = 70;

fn watch_page_fallback_embed(local_domain: &str, media_file_id: i64) -> BskyEmbed {
    // 音声（Bskyに専用embedが無い）・動画パイプライン未完了/失敗時のフォールバックリンク先は、
    // メディアファイルの直リンクではなく簡易視聴ページ（`handlers::drive::watch_media`）にする。
    // 直リンクだとブラウザがダウンロードしてしまい再生できないため（2026-07-17 マイケル指摘）。
    BskyEmbed::External {
        url: format!("https://{}/api/media/{}/watch", local_domain, media_file_id),
        title: String::new(),
        description: String::new(),
        thumb: None,
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn handle(
    actor_id: i64,
    post_id: i64,
    text: String,
    pending_media_file_id: i64,
    reply_root: Option<(String, String)>,
    reply_parent: Option<(String, String)>,
    now: DateTime<Utc>,
    ctx: Arc<JobContext>,
) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        tracing::warn!(
            "[BskyPostCommitDeferred] DB pool 未設定のためスキップ (post_id={})",
            post_id
        );
        return Ok(());
    };
    let cfg = ctx
        .delivery
        .as_ref()
        .ok_or_else(|| "配送設定未注入".to_string())?;

    let row = sqlx::query(
        "SELECT mime_type, width, height, bsky_video_cid, bsky_video_status, bsky_video_size, size, created_at
         FROM media_files WHERE id = $1",
    )
    .bind(pending_media_file_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("media_files取得失敗: {}", e))?;

    let embed = match row {
        None => {
            tracing::warn!(
                "[BskyPostCommitDeferred] media_file_id={} が見つからないため視聴ページへフォールバック post_id={}",
                pending_media_file_id, post_id
            );
            watch_page_fallback_embed(&cfg.local_domain, pending_media_file_id)
        }
        Some(row) => {
            let status: Option<String> = row.try_get("bsky_video_status").unwrap_or(None);
            let created_at: DateTime<Utc> = row
                .try_get("created_at")
                .map_err(|e| format!("created_at取得失敗: {}", e))?;

            if !matches!(status.as_deref(), Some("ready") | Some("failed")) {
                let elapsed_secs = (Utc::now() - created_at).num_seconds();
                if elapsed_secs < SETTLE_TIMEOUT_SECS {
                    return Err(format!("動画パイプライン結合待ち（経過{}秒）", elapsed_secs));
                }
                tracing::warn!(
                    "[BskyPostCommitDeferred] {}秒経過してもbsky_video_statusが確定しないためフォールバックコミット post_id={}",
                    elapsed_secs, post_id
                );
            }

            if status.as_deref() == Some("ready") {
                let video_cid: Option<String> = row.try_get("bsky_video_cid").unwrap_or(None);
                match video_cid {
                    Some(video_cid) => {
                        let mime_type: String = row.try_get("mime_type").unwrap_or_default();
                        let width: Option<i32> = row.try_get("width").unwrap_or(None);
                        let height: Option<i32> = row.try_get("height").unwrap_or(None);
                        let size: i64 = row.try_get("size").unwrap_or(0);
                        let bsky_size: Option<i64> =
                            row.try_get("bsky_video_size").unwrap_or(None);
                        // 音声を変換したグレー背景動画の解像度は
                        // crate::storage::media_probe::AUDIO_VIDEO_WIDTH/HEIGHT
                        // （convert_audio_to_gray_video が実際に生成する解像度）と必ず一致させる。
                        let is_audio = mime_type.starts_with("audio/");
                        let (embed_width, embed_height) = if is_audio {
                            (AUDIO_VIDEO_WIDTH as i32, AUDIO_VIDEO_HEIGHT as i32)
                        } else {
                            (width.unwrap_or(0), height.unwrap_or(0))
                        };
                        BskyEmbed::Video {
                            cid: video_cid,
                            mime_type: "video/mp4".to_string(),
                            size: bsky_size.unwrap_or(size),
                            width: embed_width,
                            height: embed_height,
                        }
                    }
                    None => watch_page_fallback_embed(&cfg.local_domain, pending_media_file_id),
                }
            } else {
                watch_page_fallback_embed(&cfg.local_domain, pending_media_file_id)
            }
        }
    };

    let (bsky_text, bsky_facets) =
        convert_mentions_for_bsky(&text, &cfg.local_domain, pool, ctx.ap_client.http.as_ref())
            .await;

    let bsky_reply = match (reply_root, reply_parent) {
        (Some((root_uri, root_cid)), Some((parent_uri, parent_cid))) => Some(BskyPostReply {
            root: BskyRefRecord {
                uri: root_uri,
                cid: root_cid,
            },
            parent: BskyRefRecord {
                uri: parent_uri,
                cid: parent_cid,
            },
        }),
        _ => None,
    };

    // ATPコミット用のサービス。event_txはこのジョブ専用の使い捨てチャンネルで良い
    // （account_withdraw_unfollow_all と同じ理由: subscribeReposのリアルタイム購読者には
    // 届かないが、atp_repo_eventsテーブルへの記録自体は行われるため、他のRelayが
    // 再購読すれば最終的に一貫する）。
    let (event_tx, _rx) = broadcast::channel(16);
    let atp_service = AtpCommitService::new(
        pool.clone(),
        Arc::new(event_tx),
        Arc::clone(&ctx.ap_client.http),
    );

    atp_service
        .commit_post(
            actor_id,
            post_id,
            &bsky_text,
            bsky_facets,
            Some(embed),
            now,
            bsky_reply,
        )
        .await
        .map_err(|e| format!("ATP コミット失敗: {}", e))?;

    tracing::info!("[BskyPostCommitDeferred] コミット完了 post_id={}", post_id);
    Ok(())
}

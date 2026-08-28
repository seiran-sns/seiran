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
//!
//! **ペイロード最小化と `post_id` 単位の排他ロック**: このジョブは `post_id`/
//! `pending_media_file_id` のみを持ち、本文・投稿時刻・リプライ先at_uri/at_cidは
//! `posts` テーブルから都度取得する（DBの別の場所に既にある情報をジョブのペイロードとして
//! 二重に持たない設計）。これにより、`InMemoryJobQueue`がプロセス再起動でリトライ待ち
//! ジョブを失っても、起動時リカバリ（`seiran-api` `spawn_startup_tasks`）が
//! `posts.pending_bsky_media_file_id IS NOT NULL AND at_uri IS NULL` を検出して
//! `post_id`だけから元のジョブを完全に再現できる。直前のジョブがまだ生きていれば
//! 同一`post_id`に対して複数のジョブが同時に走りうるため、`advisory_lock::try_acquire`
//! で排他制御する（詳細は`crate::advisory_lock`のドキュメント参照）。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
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

pub async fn handle(
    actor_id: i64,
    post_id: i64,
    pending_media_file_id: i64,
    ctx: Arc<JobContext>,
) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        tracing::warn!(
            "[BskyPostCommitDeferred] DB pool 未設定のためスキップ (post_id={})",
            post_id
        );
        return Ok(());
    };

    let Some(lock_conn) = crate::advisory_lock::try_acquire(pool, post_id).await? else {
        tracing::info!(
            "[BskyPostCommitDeferred] post_id={} は既に別のジョブが処理中のためスキップ",
            post_id
        );
        return Ok(());
    };

    let result = process_locked(actor_id, post_id, pending_media_file_id, pool, &ctx).await;

    crate::advisory_lock::release(lock_conn, post_id).await;

    result
}

/// `post_id` のリプライ先（`reply_to_post_id`）を辿り、Bsky reply フィールド用の
/// `(root, parent)` を組み立てる。root/parent は常に同じ値（直接の親のat_uri/at_cid）で、
/// `seiran-api::handlers::notes::delivery::resolve_reply_context` と同じ規約
/// （スレッド全体のrootを別途辿ることはしない）。
async fn resolve_reply_uris(
    pool: &PgPool,
    reply_to_post_id: Option<i64>,
) -> Result<Option<BskyPostReply>, String> {
    let Some(reply_to_post_id) = reply_to_post_id else {
        return Ok(None);
    };
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT at_uri, at_cid FROM posts WHERE id = $1")
            .bind(reply_to_post_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("親ポストのat_uri/at_cid取得失敗: {}", e))?;

    Ok(match row {
        Some((Some(uri), Some(cid))) => Some(BskyPostReply {
            root: BskyRefRecord {
                uri: uri.clone(),
                cid: cid.clone(),
            },
            parent: BskyRefRecord { uri, cid },
        }),
        _ => None,
    })
}

/// (body, created_at, reply_to_post_id, language)
type PostRow = (String, DateTime<Utc>, Option<i64>, Option<String>);

async fn process_locked(
    actor_id: i64,
    post_id: i64,
    pending_media_file_id: i64,
    pool: &PgPool,
    ctx: &JobContext,
) -> Result<(), String> {
    let cfg = ctx
        .delivery
        .as_ref()
        .ok_or_else(|| "配送設定未注入".to_string())?;

    let post_row: Option<PostRow> = sqlx::query_as(
        "SELECT body, created_at, reply_to_post_id, language FROM posts WHERE id = $1",
    )
    .bind(post_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("投稿取得失敗: {}", e))?;

    let Some((text, now, reply_to_post_id, language)) = post_row else {
        tracing::warn!(
            "[BskyPostCommitDeferred] post_id={} が見つからないため終了",
            post_id
        );
        return Ok(());
    };

    let bsky_reply = resolve_reply_uris(pool, reply_to_post_id).await?;

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
            language,
        )
        .await
        .map_err(|e| format!("ATP コミット失敗: {}", e))?;

    // 結合待ちを解消（起動時リカバリの再検出対象から外す）。commit_post成功時点で
    // posts.at_uri が設定されているはずだが、pending_bsky_media_file_id自体も明示的に
    // クリアしておく（at_uri判定だけに頼らず、意図をカラムの値としても残すため）。
    if let Err(e) = sqlx::query("UPDATE posts SET pending_bsky_media_file_id = NULL WHERE id = $1")
        .bind(post_id)
        .execute(pool)
        .await
    {
        tracing::error!(
            "[BskyPostCommitDeferred] pending_bsky_media_file_id クリア失敗 post_id={}: {}",
            post_id,
            e
        );
    }

    tracing::info!("[BskyPostCommitDeferred] コミット完了 post_id={}", post_id);
    Ok(())
}

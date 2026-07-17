//! ⑥ Bsky動画パイプライン結合キュー (`bsky_video_poll`)
//!
//! 動画添付アップロード時に `app.bsky.video.uploadVideo` へ投げたジョブの完了を
//! 待つ。1回の実行で `app.bsky.video.getJobStatus` を1回だけ叩き、未完了なら
//! `Err` を返して `WorkerEngine` の既存リトライ機構（固定3秒間隔・最大10回=30秒、
//! `crates/seiran-common/src/queue/worker.rs` の `retry_config_for`）に
//! 判断を委ねる。詳細仕様は `docs/03_multi_protocol_engine_specification.md` §12。

use std::sync::Arc;

use sqlx::Row;

use crate::atp::sign_service_auth_jwt;
use crate::queue::worker::JobContext;

const VIDEO_SERVICE_HOST: &str = "https://video.bsky.app";

pub async fn handle(media_file_id: i64, ctx: Arc<JobContext>) -> Result<(), String> {
    let pool = match &ctx.db_pool {
        Some(p) => p,
        None => {
            tracing::warn!("[BskyVideoPoll] DB pool 未設定のためスキップ (media_file_id={})", media_file_id);
            return Ok(());
        }
    };

    let row = sqlx::query(
        "SELECT mf.bsky_video_job_id, a.at_did, a.at_signing_key_pem
         FROM media_files mf
         JOIN actors a ON a.id = mf.uploaded_by_actor_id
         WHERE mf.id = $1",
    )
    .bind(media_file_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("DB取得失敗: {}", e))?;

    let Some(row) = row else {
        tracing::warn!("[BskyVideoPoll] media_file_id={} が見つかりません（終了）", media_file_id);
        return Ok(());
    };

    let job_id: Option<String> = row.try_get("bsky_video_job_id").unwrap_or(None);
    let did: Option<String> = row.try_get("at_did").unwrap_or(None);
    let pem: Option<String> = row.try_get("at_signing_key_pem").unwrap_or(None);

    let (Some(job_id), Some(did), Some(pem)) = (job_id, did, pem) else {
        tracing::info!("[BskyVideoPoll] media_file_id={} に必要な情報が無い（終了）", media_file_id);
        mark_failed(pool, media_file_id).await;
        return Ok(());
    };

    let local_domain = std::env::var("LOCAL_DOMAIN").unwrap_or_else(|_| "localhost".to_string());
    let own_pds_did = format!("did:web:{}", local_domain);

    let jwt = sign_service_auth_jwt(&pem, &did, &own_pds_did, "app.bsky.video.getJobStatus")
        .map_err(|e| format!("JWT署名失敗: {}", e))?;

    let poll_url = format!("{}/xrpc/app.bsky.video.getJobStatus?jobId={}", VIDEO_SERVICE_HOST, job_id);
    let resp = ctx
        .ap_client
        .http
        .get(&poll_url)
        .header("Authorization", format!("Bearer {}", jwt))
        .send()
        .await
        .map_err(|e| format!("getJobStatus リクエスト失敗: {}", e))?;

    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        tracing::error!("[BskyVideoPoll] getJobStatus失敗 media_file_id={} status={} body={}", media_file_id, status, body_text);
        mark_failed(pool, media_file_id).await;
        return Ok(());
    }

    // 成功時はフラット、失敗時は`jobStatus`にネストされる（実機検証で判明）。両対応。
    let parsed: serde_json::Value = serde_json::from_str(&body_text)
        .map_err(|e| format!("getJobStatus応答パース失敗: {}", e))?;
    let job_status = parsed.get("jobStatus").unwrap_or(&parsed);

    let state = job_status.get("state").and_then(|v| v.as_str()).unwrap_or("");

    match state {
        "JOB_STATE_COMPLETED" => {
            let Some(blob) = job_status.get("blob") else {
                tracing::info!("[BskyVideoPoll] 完了状態だがblobが無い media_file_id={}", media_file_id);
                mark_failed(pool, media_file_id).await;
                return Ok(());
            };
            let Some(cid) = blob.get("ref").and_then(|r| r.get("$link")).and_then(|v| v.as_str()) else {
                tracing::info!("[BskyVideoPoll] 完了状態だがblob CIDが無い media_file_id={}", media_file_id);
                mark_failed(pool, media_file_id).await;
                return Ok(());
            };
            // トランスコード後の実際のバイト列サイズ。media_files.size（アップロード時の
            // オリジナルサイズ）とは異なるため別カラムに保持する（app.bsky.embed.video の
            // size フィールドに使う。実機確認: 2,867,780→287,123 バイトのように変わる）。
            let bsky_size = blob.get("size").and_then(|v| v.as_i64());
            sqlx::query(
                "UPDATE media_files SET bsky_video_cid = $1, bsky_video_status = 'ready', bsky_video_size = $2 WHERE id = $3",
            )
            .bind(cid)
            .bind(bsky_size)
            .bind(media_file_id)
            .execute(pool)
            .await
            .map_err(|e| format!("DB更新失敗: {}", e))?;
            tracing::info!("[BskyVideoPoll] 完了 media_file_id={} cid={} size={:?}", media_file_id, cid, bsky_size);
            Ok(())
        }
        "JOB_STATE_FAILED" => {
            tracing::error!("[BskyVideoPoll] Bluesky側が失敗を報告 media_file_id={} body={}", media_file_id, body_text);
            mark_failed(pool, media_file_id).await;
            Ok(())
        }
        _ => {
            // まだ処理中。Errを返してWorkerEngineにリトライさせる。
            Err(format!("処理中(state={})", state))
        }
    }
}

async fn mark_failed(pool: &sqlx::PgPool, media_file_id: i64) {
    let _ = sqlx::query("UPDATE media_files SET bsky_video_status = 'failed' WHERE id = $1")
        .bind(media_file_id)
        .execute(pool)
        .await;
}

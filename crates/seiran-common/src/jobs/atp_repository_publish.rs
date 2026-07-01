//! ⑤ 外部 PDS ミラーリングキュー (`atp_repository_publish`)
//!
//! seiran のローカル投稿を **外部 Bluesky PDS（bsky.social 等）にミラーリング**するジョブ。
//! ローカル ATP リポジトリへのコミット（`AtpCommitService`）とは独立した処理であり、
//! 役割は bsky.social の App Password を用いた `createRecord` 呼び出しのみ。
//!
//! アクターID単位の FIFO 排他ロックで投稿順序の整合性を保証する。
//!
//! # 環境変数
//! - `ATP_HANDLE` — Bluesky ハンドル（例: `seiran.bsky.social`）
//! - `ATP_APP_PASSWORD` — App Password
//! - `ATP_PDS_URL` — PDS エンドポイント（デフォルト: `https://bsky.social`）

use std::sync::Arc;

use sqlx::Row;

use crate::atp::client::{create_atp_post, create_atp_session};
use crate::queue::worker::JobContext;

pub async fn handle(actor_id: i64, commit_type: String, ctx: Arc<JobContext>) -> Result<(), String> {
    let sem = ctx.get_actor_semaphore(actor_id).await;
    let _permit = sem
        .acquire_owned()
        .await
        .map_err(|e| format!("アクター排他ロック取得失敗: {}", e))?;

    eprintln!(
        "[Job::AtpRepositoryPublish] 開始 - actor_id: {}, commit_type: {}",
        actor_id, commit_type
    );

    match commit_type.as_str() {
        "create_post" => {
            handle_create_post(actor_id, &ctx).await?;
        }
        other => {
            eprintln!(
                "[Job::AtpRepositoryPublish] 未対応のコミットタイプ: {} (actor_id={})",
                other, actor_id
            );
        }
    }

    eprintln!(
        "[Job::AtpRepositoryPublish] 正常終了 - actor_id: {}",
        actor_id
    );
    Ok(())
}

async fn handle_create_post(actor_id: i64, ctx: &Arc<JobContext>) -> Result<(), String> {
    let pool = match &ctx.db_pool {
        Some(p) => p,
        None => {
            eprintln!("[AtpRepositoryPublish] DB pool 未設定のためスキップ");
            return Ok(());
        }
    };

    // ATP 認証情報を環境変数から取得
    let atp_handle = match std::env::var("ATP_HANDLE").ok() {
        Some(h) => h,
        None => {
            eprintln!("[AtpRepositoryPublish] ATP_HANDLE 未設定のためスキップ");
            return Ok(());
        }
    };
    let atp_password = match std::env::var("ATP_APP_PASSWORD").ok() {
        Some(p) => p,
        None => {
            eprintln!("[AtpRepositoryPublish] ATP_APP_PASSWORD 未設定のためスキップ");
            return Ok(());
        }
    };
    let pds_url = std::env::var("ATP_PDS_URL")
        .unwrap_or_else(|_| "https://bsky.social".to_string());

    // actor_id に紐付く最新の未配信ポストを取得
    let row = sqlx::query(
        "SELECT id, body, created_at FROM posts
         WHERE actor_id = $1 AND at_uri IS NULL AND deleted_at IS NULL
         ORDER BY id DESC LIMIT 1",
    )
    .bind(actor_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("ポストDB検索失敗: {}", e))?;

    let row = match row {
        Some(r) => r,
        None => {
            eprintln!("[AtpRepositoryPublish] 配信対象ポストなし (actor_id={})", actor_id);
            return Ok(());
        }
    };

    let post_id: i64 = row.try_get("id").map_err(|e| format!("id取得失敗: {}", e))?;
    let body: String = row
        .try_get("body")
        .map_err(|e| format!("body取得失敗: {}", e))?;
    let created_at: chrono::DateTime<chrono::Utc> = row
        .try_get("created_at")
        .map_err(|e| format!("created_at取得失敗: {}", e))?;

    let http = &ctx.ap_client.http;

    // PDS セッション作成
    let session = create_atp_session(http, &pds_url, &atp_handle, &atp_password).await?;

    // ATP ポスト作成
    let (at_uri, at_cid) = create_atp_post(http, &session, &pds_url, &body, created_at).await?;

    // DB に AT URI / CID を書き戻す
    sqlx::query("UPDATE posts SET at_uri = $1, at_cid = $2 WHERE id = $3")
        .bind(&at_uri)
        .bind(&at_cid)
        .bind(post_id)
        .execute(pool)
        .await
        .map_err(|e| format!("ATP URI 書き戻し失敗: {}", e))?;

    eprintln!(
        "[AtpRepositoryPublish] 外部 PDS 配信完了: post_id={}, at_uri={}",
        post_id, at_uri
    );
    Ok(())
}

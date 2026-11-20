//! 絵文字インポート（Misskey形式ZIP）ハンドラ。
//!
//! 受け取ったZIPは共有作業ディレクトリ（`seiran_common::work_dir`）へ書き出し、取り込みは
//! ジョブ（`Job::EmojiImportProcess`、`seiran_common::jobs::emoji_import`）に任せる。
//! 進捗はDBに持つので、どのプロセスに問い合わせても、再起動をまたいでも見える。
//!
//! ## API
//! - `POST /api/admin/emojis/import`        — ZIP を受け取り、インポートを開始する
//! - `GET  /api/admin/emojis/import`        — 直近のインポートの進捗を返す
//! - `POST /api/admin/emojis/import/cancel` — 実行中のインポートを止める

use axum::{
    extract::{Multipart, State},
    Extension, Json,
};
use chrono::Utc;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use seiran_common::{
    generate_snowflake_id, queue::worker::priority, repository::emoji_import, work_dir, Job,
};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportStartResponse {
    pub request_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProblem {
    pub shortcode: String,
    /// `skipped` / `failed`
    pub status: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportStatusResponse {
    /// `idle` / `preparing` / `running` / `completed` / `cancelled` / `failed`
    pub status: String,
    /// `preparing` の間（ZIPの中身を数え終わるまで）は `null`。
    pub total: Option<i32>,
    pub processed: i64,
    pub imported: i64,
    pub skipped: i64,
    pub failed: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
    pub problems: Vec<ImportProblem>,
}

const IMPORT_RUNNING: &str = "EMOJI_IMPORT_RUNNING";

/// `POST /api/admin/emojis/import`
pub async fn start_import(
    Extension(admin): Extension<AuthUser>,
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<ImportStartResponse>, ApiError> {
    // アップロードを受け切る前に断れるよう、先に確かめる（確定は create_request の一意制約）。
    if emoji_import::find_active(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .is_some()
    {
        return Err(ApiError::Conflict(IMPORT_RUNNING));
    }

    let request_id = generate_snowflake_id(Utc::now());
    let zip_path = work_dir::emoji_import_zip_path(request_id);
    let part_path = zip_path.with_extension("zip.part");

    let received = receive_zip(&mut multipart, &part_path).await;
    match received {
        Ok(true) => {}
        Ok(false) => {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(ApiError::BadRequest("ZIP_FILE_REQUIRED".to_owned()));
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(e);
        }
    }
    tokio::fs::rename(&part_path, &zip_path)
        .await
        .map_err(|e| ApiError::Internal(format!("ZIPの配置に失敗: {}", e)))?;

    let created = emoji_import::create_request(&state.db, request_id, admin.user_id, Utc::now())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !created {
        let _ = tokio::fs::remove_file(&zip_path).await;
        return Err(ApiError::Conflict(IMPORT_RUNNING));
    }

    enqueue(&state, request_id).await;

    Ok(Json(ImportStartResponse {
        request_id: request_id.to_string(),
    }))
}

/// multipart の `file` を `path` へ書き出す。`file` が無ければ `Ok(false)`。
/// ZIP 全体をメモリに持たないよう、届いた分から書いていく。
async fn receive_zip(multipart: &mut Multipart, path: &std::path::Path) -> Result<bool, ApiError> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| ApiError::Internal(format!("作業ディレクトリを作れません: {}", e)))?;
    }
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let mut file = tokio::fs::File::create(path)
            .await
            .map_err(|e| ApiError::Internal(format!("一時ファイルを作れません: {}", e)))?;
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| ApiError::BadRequest(e.to_string()))?
        {
            file.write_all(&chunk)
                .await
                .map_err(|e| ApiError::Internal(format!("一時ファイルへの書き込み失敗: {}", e)))?;
        }
        file.flush()
            .await
            .map_err(|e| ApiError::Internal(format!("一時ファイルへの書き込み失敗: {}", e)))?;
        return Ok(true);
    }
    Ok(false)
}

/// `GET /api/admin/emojis/import`
pub async fn get_import_status(
    State(state): State<AppState>,
) -> Result<Json<ImportStatusResponse>, ApiError> {
    let progress = emoji_import::find_latest_progress(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let resp = match progress {
        Some(p) => ImportStatusResponse {
            status: p.status,
            total: p.total,
            processed: p.imported + p.skipped + p.failed,
            imported: p.imported,
            skipped: p.skipped,
            failed: p.failed,
            error_reason: p.error_reason,
            error_detail: p.error_detail,
            problems: p
                .problems
                .into_iter()
                .map(|pr| ImportProblem {
                    shortcode: pr.shortcode,
                    status: pr.status,
                    reason: pr.reason,
                    detail: pr.detail,
                })
                .collect(),
        },
        None => ImportStatusResponse {
            status: "idle".to_owned(),
            total: None,
            processed: 0,
            imported: 0,
            skipped: 0,
            failed: 0,
            error_reason: None,
            error_detail: None,
            problems: Vec::new(),
        },
    };
    Ok(Json(resp))
}

/// `POST /api/admin/emojis/import/cancel`
///
/// 状態を `cancelled` にするだけで、ZIP の後片付けは次に動くジョブが行う
/// （取り込み中のジョブが読んでいるファイルをここで消さないため）。
pub async fn cancel_import(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cancelled = emoji_import::cancel_active(&state.db, Utc::now())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let Some(request_id) = cancelled else {
        return Err(ApiError::NotFound("EMOJI_IMPORT_NOT_RUNNING"));
    };
    // ジョブがもう積まれていない（準備中に止まっていた等）場合にも ZIP が片付くよう、
    // 一度積んでおく。重なってもジョブ側の advisory lock で1つしか動かない。
    enqueue(&state, request_id).await;
    Ok(Json(serde_json::json!({"status": "cancelled"})))
}

async fn enqueue(state: &AppState, request_id: i64) {
    if let Err(e) = state
        .job_queue
        .enqueue(Job::EmojiImportProcess { request_id }, priority::LOW)
        .await
    {
        tracing::error!(
            "[job] EmojiImportProcess enqueue 失敗 (request_id={}): {}",
            request_id,
            e
        );
    }
}

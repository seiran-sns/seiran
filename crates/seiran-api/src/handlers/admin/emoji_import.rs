//! 絵文字インポート（Misskey ZIP エクスポート形式）ハンドラ（#50）
//!
//! ## API
//! - `POST /api/admin/emojis/import`  — ZIP を受け取り、非同期ジョブを開始して job_id を返す
//! - `GET  /api/admin/emojis/import/:job_id` — ジョブの進捗を返す

use std::io::{Cursor, Read};
use std::sync::Arc;

use axum::{
    extract::{Multipart, Path, State},
    http::HeaderMap,
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use seiran_common::{generate_snowflake_id, prepare_image, MediaKind};

use crate::error::ApiError;
use crate::handlers::media_store;
use crate::middleware::require_admin;
use crate::AppState;

// ─── ジョブ状態 ─────────────────────────────────────────────────────────

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportJobStatus {
    pub job_id: String,
    pub total: usize,
    pub processed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub done: bool,
    pub errors: Vec<String>,
}

// ─── Misskey meta.json 構造 ──────────────────────────────────────────────

#[derive(Deserialize)]
struct MetaJson {
    emojis: Vec<MetaEntry>,
}

#[derive(Deserialize)]
struct MetaEntry {
    #[serde(rename = "fileName")]
    file_name: String,
    emoji: EmojiMeta,
}

#[derive(Deserialize)]
struct EmojiMeta {
    name: String,
    category: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    license: Option<String>,
}

// ─── レスポンス ──────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportStartResponse {
    pub job_id: String,
    pub total: usize,
}

// ─── ハンドラ ────────────────────────────────────────────────────────────

/// `POST /api/admin/emojis/import`
pub async fn start_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<ImportStartResponse>, ApiError> {
    require_admin(&headers, &state.local_auth, &*state.users).await?;

    // ZIP バイト列を取得
    let mut zip_bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?
    {
        if field.name() == Some("file") {
            zip_bytes = Some(
                field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::BadRequest(e.to_string()))?
                    .to_vec(),
            );
        }
    }
    let zip_bytes = zip_bytes
        .ok_or_else(|| ApiError::BadRequest("ZIPファイルが含まれていません".to_owned()))?;

    // meta.json を解析して総件数を確認（blocking I/O）
    let zip_clone = zip_bytes.clone();
    let meta: MetaJson = tokio::task::spawn_blocking(move || {
        let cursor = Cursor::new(zip_clone);
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|e| format!("ZIP 解析エラー: {}", e))?;
        let mut meta_file = archive
            .by_name("meta.json")
            .map_err(|_| "meta.json が見つかりません".to_owned())?;
        let mut buf = String::new();
        meta_file
            .read_to_string(&mut buf)
            .map_err(|e| format!("meta.json 読み込みエラー: {}", e))?;
        serde_json::from_str::<MetaJson>(&buf).map_err(|e| format!("meta.json 解析エラー: {}", e))
    })
    .await
    .map_err(|_| ApiError::Internal("タスク実行エラー".to_owned()))?
    .map_err(ApiError::BadRequest)?;

    let total = meta.emojis.len();
    let job_id = Uuid::new_v4().to_string();

    state.emoji_import_jobs.insert(
        job_id.clone(),
        ImportJobStatus {
            job_id: job_id.clone(),
            total,
            processed: 0,
            skipped: 0,
            failed: 0,
            done: false,
            errors: Vec::new(),
        },
    );

    // バックグラウンドジョブを起動
    let state_clone = state.clone();
    let job_id_clone = job_id.clone();
    tokio::spawn(async move {
        run_import(state_clone, job_id_clone, zip_bytes, meta).await;
    });

    Ok(Json(ImportStartResponse { job_id, total }))
}

/// `GET /api/admin/emojis/import/:job_id`
pub async fn get_import_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<ImportJobStatus>, ApiError> {
    require_admin(&headers, &state.local_auth, &*state.users).await?;

    let status = state
        .emoji_import_jobs
        .get(&job_id)
        .map(|s| s.clone())
        .ok_or(ApiError::NotFound("JOB_NOT_FOUND"))?;

    Ok(Json(status))
}

// ─── インポート処理本体 ──────────────────────────────────────────────────

async fn run_import(state: AppState, job_id: String, zip_bytes: Vec<u8>, meta: MetaJson) {
    let zip_arc = Arc::new(zip_bytes);

    for entry in &meta.emojis {
        let shortcode = entry.emoji.name.trim().to_owned();
        if shortcode.is_empty() {
            update_job(&state, &job_id, |s| {
                s.skipped += 1;
                s.errors.push("空のショートコードをスキップ".to_owned());
            });
            continue;
        }

        // 既存ショートコードのスキップ
        let exists: bool = state.emojis.exists_by_shortcode(&shortcode).await.unwrap_or(false);

        if exists {
            update_job(&state, &job_id, |s| {
                s.skipped += 1;
            });
            continue;
        }

        // ZIP から画像バイト列を取得（blocking）
        let file_name = entry.file_name.clone();
        let zip_arc2 = Arc::clone(&zip_arc);
        let image_bytes = match tokio::task::spawn_blocking(move || {
            let cursor = Cursor::new(zip_arc2.as_slice());
            let mut archive = zip::ZipArchive::new(cursor)?;
            let mut file = archive.by_name(&file_name)?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            Ok::<Vec<u8>, zip::result::ZipError>(buf)
        })
        .await
        {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => {
                update_job(&state, &job_id, |s| {
                    s.failed += 1;
                    s.errors.push(format!(":{}:  ZIP 取得エラー: {}", shortcode, e));
                });
                continue;
            }
            Err(_) => {
                update_job(&state, &job_id, |s| {
                    s.failed += 1;
                    s.errors.push(format!(":{}:  タスクパニック", shortcode));
                });
                continue;
            }
        };

        // 画像処理（Exif整理・Orientation補正・WebP変換）
        let pipeline = match prepare_image(&image_bytes, MediaKind::Emoji) {
            Ok(p) => p,
            Err(e) => {
                update_job(&state, &job_id, |s| {
                    s.failed += 1;
                    s.errors.push(format!(":{}:  画像処理エラー: {}", shortcode, e));
                });
                continue;
            }
        };

        // 重複排除チェック → S3 アップロード → DB 記録
        let media_file_id = match media_store::store_image(&state, pipeline, None).await {
            Ok(outcome) => outcome.record.id,
            Err(e) => {
                update_job(&state, &job_id, |s| {
                    s.failed += 1;
                    s.errors.push(format!(":{}:  {}", shortcode, e));
                });
                continue;
            }
        };

        // タグ: aliases を正規化
        let tags: Vec<String> = entry
            .emoji
            .aliases
            .iter()
            .map(|a| a.trim().to_owned())
            .filter(|a| !a.is_empty() && !a.chars().any(char::is_whitespace))
            .collect();

        let category = entry.emoji.category.clone().filter(|c| !c.is_empty());
        let license = entry.emoji.license.clone().filter(|l| !l.is_empty());

        let emoji_id = generate_snowflake_id(Utc::now());
        let result = state
            .emojis
            .insert_if_absent(emoji_id, &shortcode, media_file_id, category.as_deref(), &tags, license.as_deref())
            .await;

        match result {
            Ok(false) => {
                update_job(&state, &job_id, |s| { s.skipped += 1; });
            }
            Ok(true) => {
                update_job(&state, &job_id, |s| { s.processed += 1; });
            }
            Err(e) => {
                update_job(&state, &job_id, |s| {
                    s.failed += 1;
                    s.errors.push(format!(":{}:  絵文字登録エラー: {}", shortcode, e));
                });
            }
        }
    }

    // ジョブ完了
    update_job(&state, &job_id, |s| { s.done = true; });
    tracing::info!(
        "[emoji-import] job_id={} 完了: processed={} skipped={} failed={}",
        job_id,
        state.emoji_import_jobs.get(&job_id).map(|s| s.processed).unwrap_or(0),
        state.emoji_import_jobs.get(&job_id).map(|s| s.skipped).unwrap_or(0),
        state.emoji_import_jobs.get(&job_id).map(|s| s.failed).unwrap_or(0),
    );
}

fn update_job<F: FnOnce(&mut ImportJobStatus)>(state: &AppState, job_id: &str, f: F) {
    if let Some(mut s) = state.emoji_import_jobs.get_mut(job_id) {
        f(&mut s);
    }
}

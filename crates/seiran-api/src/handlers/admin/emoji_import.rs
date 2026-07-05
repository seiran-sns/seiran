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

use seiran_common::{
    generate_snowflake_id,
    process_image, MediaKind,
    repository::CreateMediaFile,
    select_provider, SelectorError, S3StorageClient,
};

use crate::error::ApiError;
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
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM custom_emojis WHERE shortcode = $1)",
        )
        .bind(&shortcode)
        .fetch_one(&state.db)
        .await
        .unwrap_or(false);

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

        // 画像処理
        let processed = match process_image(&image_bytes, MediaKind::Emoji) {
            Ok(p) => p,
            Err(e) => {
                update_job(&state, &job_id, |s| {
                    s.failed += 1;
                    s.errors.push(format!(":{}:  画像処理エラー: {}", shortcode, e));
                });
                continue;
            }
        };

        // 重複排除: sha256+blurhash が既存ならメディアファイルを再利用
        let media_file_id = match state
            .media_files
            .find_by_sha256_and_blurhash(&processed.sha256, &processed.blurhash)
            .await
        {
            Ok(Some(existing)) => existing.id,
            _ => {
                // ストレージ選択 → S3 アップロード → DB 記録
                let provider = match select_provider(state.storage_providers.as_ref(), processed.size).await {
                    Ok(p) => p,
                    Err(e) => {
                        let msg = match e {
                            SelectorError::NoAvailableProvider => "ストレージプロバイダー未設定".to_owned(),
                            SelectorError::QuotaExceeded => "ストレージ容量超過".to_owned(),
                            SelectorError::Db(e) => e.to_string(),
                        };
                        update_job(&state, &job_id, |s| {
                            s.failed += 1;
                            s.errors.push(format!(":{}:  {}", shortcode, msg));
                        });
                        continue;
                    }
                };

                let ext = match processed.mime_type.as_str() {
                    "image/jpeg" => "jpg",
                    "image/png" => "png",
                    "image/gif" => "gif",
                    "image/avif" => "avif",
                    _ => "webp",
                };
                let storage_key = format!("media/{}.{}", Uuid::new_v4(), ext);
                let s3 = S3StorageClient::new(&provider);
                match s3.put(&storage_key, processed.data.clone(), &processed.mime_type).await {
                    Ok(_) => {}
                    Err(e) => {
                        update_job(&state, &job_id, |s| {
                            s.failed += 1;
                            s.errors.push(format!(":{}:  S3 アップロードエラー: {}", shortcode, e));
                        });
                        continue;
                    }
                }

                let file_id = generate_snowflake_id(Utc::now());
                match state
                    .media_files
                    .insert(CreateMediaFile {
                        id: file_id,
                        storage_provider_id: provider.id,
                        sha256: processed.sha256,
                        blurhash: processed.blurhash,
                        size: processed.size,
                        width: processed.width as i32,
                        height: processed.height as i32,
                        mime_type: processed.mime_type,
                        storage_key,
                        uploaded_by_actor_id: None,
                    })
                    .await
                {
                    Ok(_) => file_id,
                    Err(e) => {
                        update_job(&state, &job_id, |s| {
                            s.failed += 1;
                            s.errors.push(format!(":{}:  DB 記録エラー: {}", shortcode, e));
                        });
                        continue;
                    }
                }
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
        let result = sqlx::query(
            "INSERT INTO custom_emojis (id, shortcode, media_file_id, category, tags, license)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (shortcode) DO NOTHING",
        )
        .bind(emoji_id)
        .bind(&shortcode)
        .bind(media_file_id)
        .bind(&category)
        .bind(&tags)
        .bind(&license)
        .execute(&state.db)
        .await;

        match result {
            Ok(r) if r.rows_affected() == 0 => {
                update_job(&state, &job_id, |s| { s.skipped += 1; });
            }
            Ok(_) => {
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
    eprintln!(
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

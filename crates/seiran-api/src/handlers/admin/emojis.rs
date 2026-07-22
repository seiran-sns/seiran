use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use seiran_common::generate_snowflake_id;
use seiran_common::repository::EmojiRow;

use crate::AppState;
use crate::error::ApiError;
use crate::middleware::require_admin;

// ─── レスポンス DTO ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct EmojiResponse {
    pub id: String,
    pub shortcode: String,
    pub media_file_id: String,
    pub category: Option<String>,
    /// タグ（#49）。ピッカーの部分一致対象。
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    /// 画像プレビュー用 URL。`list_emojis` のみ解決済み、`create_emoji` の直後レスポンスは
    /// `None`（呼び出し元は登録後に一覧を再取得するため実害はない）。
    pub url: Option<String>,
}

impl From<EmojiRow> for EmojiResponse {
    fn from(r: EmojiRow) -> Self {
        Self {
            id: r.id.to_string(),
            shortcode: r.shortcode,
            media_file_id: r.media_file_id.to_string(),
            category: r.category,
            tags: r.tags,
            created_at: r.created_at,
            url: r.url,
        }
    }
}

// ─── リクエスト DTO ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateEmojiRequest {
    pub shortcode: String,
    /// JS Number の 53bit 精度では snowflake ID を正確に表現できないため文字列で受け取る。
    pub media_file_id: String,
    pub category: Option<String>,
    /// タグ（#49）。省略時は空。
    pub tags: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub struct UpdateEmojiRequest {
    /// いずれも省略時は現在値を保持する。
    pub category: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// タグを正規化する（#49）。
/// トリム → 空要素除去 → 重複除去。ホワイトスペースを含むタグは不正として弾く。
fn normalize_tags(tags: &[String]) -> Result<Vec<String>, ApiError> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t = t.trim();
        if t.is_empty() {
            continue;
        }
        if t.chars().any(char::is_whitespace) {
            return Err(ApiError::BadRequest("INVALID_TAG".to_owned()));
        }
        let t = t.to_string();
        if !out.contains(&t) {
            out.push(t);
        }
    }
    Ok(out)
}

// ─── エラー変換 ───────────────────────────────────────────────────────────

fn map_emoji_db_error(e: sqlx::Error) -> ApiError {
    if let sqlx::Error::Database(ref db_err) = e {
        // PostgreSQL unique violation: error code 23505
        if db_err.code().as_deref() == Some("23505") {
            return ApiError::Conflict("SHORTCODE_TAKEN");
        }
        // foreign key violation: error code 23503（media_file_id が media_files に存在しない）
        if db_err.code().as_deref() == Some("23503") {
            return ApiError::BadRequest("MEDIA_FILE_NOT_FOUND".to_owned());
        }
    }
    ApiError::Internal(e.to_string())
}

// ─── ハンドラ ─────────────────────────────────────────────────────────────

/// GET /api/admin/emojis
pub async fn list_emojis(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Vec<EmojiResponse>>, ApiError> {
    require_admin(&headers, &state.local_auth, state.users.as_ref()).await?;

    let rows = state
        .emojis
        .list_all()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// POST /api/admin/emojis
pub async fn create_emoji(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateEmojiRequest>,
) -> Result<Json<EmojiResponse>, ApiError> {
    require_admin(&headers, &state.local_auth, state.users.as_ref()).await?;

    if req.shortcode.is_empty()
        || !req.shortcode.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        return Err(ApiError::BadRequest("INVALID_SHORTCODE".to_owned()));
    }

    let media_file_id: i64 = req
        .media_file_id
        .parse()
        .map_err(|_| ApiError::BadRequest("INVALID_MEDIA_FILE_ID".to_owned()))?;

    let tags = normalize_tags(req.tags.as_deref().unwrap_or(&[]))?;
    let id = generate_snowflake_id(Utc::now());

    let row = state
        .emojis
        .insert(id, &req.shortcode, media_file_id, req.category.as_deref(), &tags)
        .await
        .map_err(map_emoji_db_error)?;

    Ok(Json(row.into()))
}

/// PATCH /api/admin/emojis/:id — カテゴリ・タグを更新する（#49）。
pub async fn update_emoji(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateEmojiRequest>,
) -> Result<Json<EmojiResponse>, ApiError> {
    require_admin(&headers, &state.local_auth, state.users.as_ref()).await?;

    let tags = match req.tags {
        Some(ref t) => Some(normalize_tags(t)?),
        None => None,
    };

    let row = state
        .emojis
        .update(id, req.category.as_deref(), tags.as_deref())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    row.map(|r| Json(r.into()))
        .ok_or(ApiError::NotFound("EMOJI_NOT_FOUND"))
}

/// DELETE /api/admin/emojis/:id
pub async fn delete_emoji(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    require_admin(&headers, &state.local_auth, state.users.as_ref()).await?;

    let deleted = state
        .emojis
        .delete(id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if !deleted {
        return Err(ApiError::NotFound("EMOJI_NOT_FOUND"));
    }

    Ok(StatusCode::NO_CONTENT)
}

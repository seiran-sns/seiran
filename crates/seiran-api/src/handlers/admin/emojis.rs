use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use seiran_common::generate_snowflake_id;

use crate::AppState;
use crate::error::ApiError;
use crate::middleware::require_admin;

// ─── DB 行 ────────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct EmojiRow {
    id: i64,
    shortcode: String,
    media_file_id: i64,
    category: Option<String>,
    created_at: DateTime<Utc>,
}

// ─── レスポンス DTO ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct EmojiResponse {
    pub id: String,
    pub shortcode: String,
    pub media_file_id: String,
    pub category: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<EmojiRow> for EmojiResponse {
    fn from(r: EmojiRow) -> Self {
        Self {
            id: r.id.to_string(),
            shortcode: r.shortcode,
            media_file_id: r.media_file_id.to_string(),
            category: r.category,
            created_at: r.created_at,
        }
    }
}

// ─── リクエスト DTO ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateEmojiRequest {
    pub shortcode: String,
    pub media_file_id: i64,
    pub category: Option<String>,
}

// ─── エラー変換 ───────────────────────────────────────────────────────────

fn map_emoji_db_error(e: sqlx::Error) -> ApiError {
    if let sqlx::Error::Database(ref db_err) = e {
        // PostgreSQL unique violation: error code 23505
        if db_err.code().as_deref() == Some("23505") {
            return ApiError::Conflict("SHORTCODE_TAKEN");
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

    let rows: Vec<EmojiRow> = sqlx::query_as::<_, EmojiRow>(
        "SELECT id, shortcode, media_file_id, category, created_at
         FROM custom_emojis
         ORDER BY id",
    )
    .fetch_all(&state.db)
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

    let id = generate_snowflake_id(Utc::now());

    let row: EmojiRow = sqlx::query_as::<_, EmojiRow>(
        "INSERT INTO custom_emojis (id, shortcode, media_file_id, category)
         VALUES ($1, $2, $3, $4)
         RETURNING id, shortcode, media_file_id, category, created_at",
    )
    .bind(id)
    .bind(&req.shortcode)
    .bind(req.media_file_id)
    .bind(req.category.as_deref())
    .fetch_one(&state.db)
    .await
    .map_err(map_emoji_db_error)?;

    Ok(Json(row.into()))
}

/// DELETE /api/admin/emojis/:id
pub async fn delete_emoji(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    require_admin(&headers, &state.local_auth, state.users.as_ref()).await?;

    let result = sqlx::query("DELETE FROM custom_emojis WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("EMOJI_NOT_FOUND"));
    }

    Ok(StatusCode::NO_CONTENT)
}

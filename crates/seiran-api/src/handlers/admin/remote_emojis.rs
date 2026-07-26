//! リモートサーバー由来のカスタム絵文字カタログの一覧・インポート（#73）
//!
//! ## API
//! - `GET  /api/admin/emojis/remote?keyword=...` — AP受信で見つけたリモート絵文字の一覧
//! - `POST /api/admin/emojis/remote/import` — 指定の画像URLを取り込み、ローカルの
//!   カスタム絵文字として登録する

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use seiran_common::{generate_snowflake_id, prepare_image, MediaKind};

use super::emojis::{normalize_license, normalize_tags, validate_category, validate_shortcode, EmojiResponse};
use crate::error::ApiError;
use crate::handlers::media_proxy::fetch_validated;
use crate::handlers::media_store;
use crate::middleware::require_admin;
use crate::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEmojiResponse {
    pub id: String,
    pub shortcode: String,
    pub domain: String,
    pub image_url: String,
    pub tags: Vec<String>,
    pub license: Option<String>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

impl From<seiran_common::repository::RemoteEmojiRow> for RemoteEmojiResponse {
    fn from(r: seiran_common::repository::RemoteEmojiRow) -> Self {
        Self {
            id: r.id.to_string(),
            shortcode: r.shortcode,
            domain: r.domain,
            image_url: r.image_url,
            tags: r.tags,
            license: r.license,
            first_seen_at: r.first_seen_at,
            last_seen_at: r.last_seen_at,
        }
    }
}

#[derive(Deserialize)]
pub struct ListRemoteEmojisQuery {
    pub keyword: Option<String>,
}

/// `GET /api/admin/emojis/remote`
pub async fn list_remote_emojis(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(query): Query<ListRemoteEmojisQuery>,
) -> Result<Json<Vec<RemoteEmojiResponse>>, ApiError> {
    require_admin(&headers, &state.local_auth, state.app_tokens.as_ref(), state.users.as_ref()).await?;

    let keyword = query.keyword.as_deref().filter(|k| !k.trim().is_empty());
    let rows = state
        .remote_emojis
        .list(keyword)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

#[derive(Deserialize)]
pub struct ImportRemoteEmojiRequest {
    /// ローカル登録時の shortcode（通常はリモートの shortcode をそのまま渡す）。
    pub shortcode: String,
    /// 取り込む画像の URL（`remote_emojis.image_url` またはNoteCard表示中の絵文字画像URL）。
    pub image_url: String,
    pub category: Option<String>,
    pub tags: Option<Vec<String>>,
    /// 省略時はリモートで設定されていたライセンス文言をフロント側で初期値として渡す想定。
    pub license: Option<String>,
}

fn map_emoji_db_error(e: sqlx::Error) -> ApiError {
    if let sqlx::Error::Database(ref db_err) = e {
        if db_err.code().as_deref() == Some("23505") {
            return ApiError::Conflict("SHORTCODE_TAKEN");
        }
    }
    ApiError::Internal(e.to_string())
}

/// `POST /api/admin/emojis/remote/import`
pub async fn import_remote_emoji(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<ImportRemoteEmojiRequest>,
) -> Result<Json<EmojiResponse>, ApiError> {
    require_admin(&headers, &state.local_auth, state.app_tokens.as_ref(), state.users.as_ref()).await?;

    validate_shortcode(&req.shortcode)?;
    if let Some(ref c) = req.category {
        validate_category(c)?;
    }

    let tags = normalize_tags(req.tags.as_deref().unwrap_or(&[]))?;
    let license = match req.license {
        Some(ref l) if !l.trim().is_empty() => Some(normalize_license(l)?),
        _ => None,
    };

    let (image_bytes, _content_type) = fetch_validated(&req.image_url, &["image/"]).await?;
    let pipeline = prepare_image(&image_bytes, MediaKind::Emoji)
        .map_err(|e| ApiError::BadRequest(format!("画像処理エラー: {e}")))?;
    let media_file_id = media_store::store_image(&state, pipeline, None).await?.record.id;

    let id = generate_snowflake_id(Utc::now());
    let row = state
        .emojis
        .insert(
            id,
            &req.shortcode,
            media_file_id,
            req.category.as_deref(),
            &tags,
            license.as_deref(),
        )
        .await
        .map_err(map_emoji_db_error)?;

    Ok(Json(row.into()))
}

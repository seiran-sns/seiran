//! メディアのアップロード（`POST /api/v1/media`・`/api/v2/media`）と取得。
//!
//! アップロードはカスタム API・Misskey 互換 API と同じ `drive::create_drive_file` に委譲する
//! （multipart のファイルフィールド名はどちらも `file`）。変換は同期で済むので、Mastodon
//! 本家が非同期処理中に返す `202` は使わず常に `200` で URL 付きの添付を返す。
//! 代替テキスト（`description`）は seiran が保存しないため受け取っても無視し、`null` を返す。

use axum::{
    extract::{Multipart, Path, State},
    http::HeaderMap,
    Json,
};

use crate::error::ApiError;
use crate::handlers::drive::{build_public_url, create_drive_file, DriveFileResponse};
use crate::middleware::AuthedUser;
use crate::AppState;

use super::types::{MastodonMediaAttachment, MastodonMediaMeta, MastodonMediaMetaSize};

fn media_kind(mime_type: &str, is_animated_image: bool) -> &'static str {
    if is_animated_image {
        "gifv"
    } else if mime_type.starts_with("image/") {
        "image"
    } else if mime_type.starts_with("video/") {
        "video"
    } else if mime_type.starts_with("audio/") {
        "audio"
    } else {
        "unknown"
    }
}

struct MediaSummary<'a> {
    id: i64,
    url: String,
    thumbnail_url: Option<String>,
    mime_type: &'a str,
    width: Option<i32>,
    height: Option<i32>,
    blurhash: Option<String>,
    is_animated_image: bool,
}

fn to_attachment(m: MediaSummary<'_>) -> MastodonMediaAttachment {
    let original = match (m.width, m.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some(MastodonMediaMetaSize {
            width: w,
            height: h,
            size: format!("{w}x{h}"),
            aspect: f64::from(w) / f64::from(h),
        }),
        _ => None,
    };
    MastodonMediaAttachment {
        id: m.id.to_string(),
        kind: media_kind(m.mime_type, m.is_animated_image),
        preview_url: m.thumbnail_url.unwrap_or_else(|| m.url.clone()),
        url: m.url,
        remote_url: None,
        meta: MastodonMediaMeta {
            small: original.clone(),
            original,
        },
        description: None,
        blurhash: m.blurhash,
    }
}

fn from_drive_response(r: &DriveFileResponse) -> Result<MastodonMediaAttachment, ApiError> {
    Ok(to_attachment(MediaSummary {
        id: r
            .id
            .parse()
            .map_err(|_| ApiError::Internal(format!("不正なメディアID: {}", r.id)))?,
        url: r.url.clone(),
        thumbnail_url: r.thumbnail_url.clone(),
        mime_type: &r.mime_type,
        width: r.width.map(|w| w as i32),
        height: r.height.map(|h| h as i32),
        blurhash: r.blurhash.clone(),
        is_animated_image: r.is_animated_image,
    }))
}

/// POST /api/v1/media・/api/v2/media
pub async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<Json<MastodonMediaAttachment>, ApiError> {
    let Json(file) = create_drive_file(State(state), headers, multipart).await?;
    Ok(Json(from_drive_response(&file)?))
}

/// GET /api/v1/media/:id・PUT /api/v1/media/:id（代替テキスト等の更新は保存しないので、
/// どちらも現在の添付を返すだけ）。アップロード者では絞らない（同一内容のファイルは
/// 重複排除で先にアップロードした人の行を共有するため、絞ると自分のアップロードが見えなくなる。
/// 返すのは元々公開 URL の情報だけ）。
pub async fn show(
    _user: AuthedUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MastodonMediaAttachment>, ApiError> {
    let media_id: i64 = id
        .parse()
        .map_err(|_| ApiError::NotFound("RECORD_NOT_FOUND"))?;
    let file = state
        .media_files
        .find_by_id(media_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("RECORD_NOT_FOUND"))?;
    let url = build_public_url(
        state.storage_providers.as_ref(),
        file.storage_provider_id,
        &file.storage_key,
    )
    .await;
    let thumbnail_url = match &file.thumbnail_key {
        Some(key) => Some(
            build_public_url(
                state.storage_providers.as_ref(),
                file.storage_provider_id,
                key,
            )
            .await,
        ),
        None => None,
    };
    Ok(Json(to_attachment(MediaSummary {
        id: file.id,
        url,
        thumbnail_url,
        mime_type: &file.mime_type,
        width: file.width,
        height: file.height,
        blurhash: file.blurhash.clone(),
        is_animated_image: file.is_animated_image,
    })))
}

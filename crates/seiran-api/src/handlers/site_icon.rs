use std::io::Cursor;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderValue, Response, StatusCode},
};
use image::{imageops::FilterType, DynamicImage, ImageFormat};

use super::media_proxy::fetch_validated;
use crate::error::ApiError;
use crate::AppState;

/// `GET /api/site-icon/:sha256/:size`
///
/// サイトアイコン（`site_settings.site_icon_sha256`）を favicon/PWAアイコン用の
/// 指定サイズPNGにリサイズして返す。URLがsha256を含むcontent-addressableな形式のため
/// `Cache-Control: immutable` を付与できる。
///
/// アニメーション画像（GIF/APNG/WebPアニメ）は管理者が意図した演出とみなし、
/// リサイズせず元バイト列のまま返す（`image` crateはアニメーションPNG/WebPの
/// 書き出しに対応していないため、リサイズ自体もできない）。
pub async fn site_icon(
    State(state): State<AppState>,
    Path((sha256, size)): Path<(String, String)>,
) -> Result<Response<Body>, ApiError> {
    let size: u32 = size
        .trim_end_matches(".png")
        .parse()
        .ok()
        .filter(|&n| n > 0 && n <= 1024)
        .ok_or(ApiError::BadRequest("INVALID_SITE_ICON_SIZE".to_string()))?;

    let resolved = state
        .media_files
        .resolve_public_by_sha256(&sha256)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("SITE_ICON_NOT_FOUND"))?;

    let (bytes, content_type) = fetch_validated(&resolved.url, &["image/"]).await?;

    let body = if resolved.is_animated_image {
        bytes.to_vec()
    } else {
        let img = image::load_from_memory(&bytes).map_err(|e| ApiError::Internal(e.to_string()))?;
        encode_png(&img, size).map_err(|e| ApiError::Internal(e.to_string()))?
    };
    let content_type = if resolved.is_animated_image {
        content_type
    } else {
        "image/png".to_string()
    };

    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    Ok(response)
}

fn encode_png(img: &DynamicImage, size: u32) -> Result<Vec<u8>, image::ImageError> {
    let resized = img.resize_exact(size, size, FilterType::Lanczos3);
    let mut bytes = Cursor::new(Vec::new());
    resized.write_to(&mut bytes, ImageFormat::Png)?;
    Ok(bytes.into_inner())
}

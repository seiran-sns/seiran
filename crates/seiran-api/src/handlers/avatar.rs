use axum::{
    extract::Path,
    http::{header, HeaderValue},
    response::IntoResponse,
};

/// GET /api/avatars/:actor_id — 未設定アバターの決定論的 PNG。
pub async fn fallback_avatar(Path(actor_id): Path<i64>) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("image/png")),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            ),
        ],
        seiran_common::avatar::fallback_avatar_png(actor_id),
    )
}

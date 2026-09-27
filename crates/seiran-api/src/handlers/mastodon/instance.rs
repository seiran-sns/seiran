//! インスタンス情報・カスタム絵文字と、クライアントが起動時に呼ぶ周辺エンドポイント。

use std::sync::OnceLock;

use axum::{
    extract::State,
    http::{header, HeaderValue},
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};

use seiran_common::version::SERVER_VERSION;

use crate::error::ApiError;
use crate::handlers::emojis::fetch_public_emojis;
use crate::handlers::notes::validation::strip_html_tags;
use crate::handlers::notes::BSKY_MAX_TEXT_GRAPHEMES;
use crate::AppState;

use super::types::MastodonEmoji;

/// クライアントは `version` で機能の有無を判断する（例: 4.5 以上なら引用投稿 UI を出す）。
/// 先頭を Mastodon 互換のバージョンにし、括弧内に実体を書く（Pleroma 等と同じ慣習）。
fn compatible_version() -> String {
    format!("4.5.0 (compatible; seiran {SERVER_VERSION})")
}

/// Mastodon API のバージョン番号（`api_versions.mastodon`）。4.5 相当。
const MASTODON_API_VERSION: i64 = 7;

/// 1投稿あたりの添付上限として案内する値。サーバー自体は10件まで受ける
/// （`validate_attachment_ids`）が、Bsky の画像は1投稿4枚までなので、両方へ配送できる数を案内する。
const MAX_MEDIA_ATTACHMENTS: i64 = 4;

const SUPPORTED_MIME_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "image/avif",
    "video/mp4",
    "video/quicktime",
    "video/webm",
    "audio/mpeg",
    "audio/mp4",
    "audio/ogg",
    "audio/wav",
];

struct SiteInfo {
    title: String,
    description: String,
    thumbnail: Option<String>,
}

async fn site_info(state: &AppState) -> Result<SiteInfo, ApiError> {
    let settings = state
        .site_settings
        .get_all()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let get = |k: &str| settings.get(k).cloned().unwrap_or_default();
    let title = strip_html_tags(&get("site_name"));
    Ok(SiteInfo {
        title: if title.is_empty() {
            "seiran".to_owned()
        } else {
            title
        },
        description: strip_html_tags(&get("site_description")),
        thumbnail: Some(get("site_icon_url")).filter(|s| !s.is_empty()),
    })
}

fn statuses_configuration() -> Value {
    json!({
        "max_characters": BSKY_MAX_TEXT_GRAPHEMES,
        "max_media_attachments": MAX_MEDIA_ATTACHMENTS,
        "characters_reserved_per_url": 23,
    })
}

fn media_configuration() -> Value {
    json!({
        "supported_mime_types": SUPPORTED_MIME_TYPES,
        "image_size_limit": 10 * 1024 * 1024,
        "image_matrix_limit": 16_777_216,
        "video_size_limit": 10 * 1024 * 1024,
        "video_frame_rate_limit": 60,
        "video_matrix_limit": 8_294_400,
        "description_limit": 1500,
    })
}

fn polls_configuration() -> Value {
    json!({
        "max_options": 10,
        "max_characters_per_option": 100,
        "min_expiration": 300,
        "max_expiration": 2_629_746,
    })
}

/// GET /api/v1/instance
pub async fn instance_v1(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let site = site_info(&state).await?;
    let (status_count, user_count) = seiran_common::repository::post::local_stats(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let domain = state.local_domain.to_string();
    Ok(Json(json!({
        "uri": domain,
        "title": site.title,
        "short_description": site.description,
        "description": site.description,
        "email": "",
        "version": compatible_version(),
        "urls": { "streaming_api": format!("wss://{domain}") },
        "stats": {
            "user_count": user_count,
            "status_count": status_count,
            "domain_count": 0,
        },
        "thumbnail": site.thumbnail,
        "languages": ["ja", "en"],
        "registrations": true,
        "approval_required": false,
        "invites_enabled": false,
        "configuration": {
            "accounts": { "max_featured_tags": 0 },
            "statuses": statuses_configuration(),
            "media_attachments": media_configuration(),
            "polls": polls_configuration(),
        },
        "contact_account": null,
        "rules": [],
    })))
}

/// GET /api/v2/instance
pub async fn instance_v2(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let site = site_info(&state).await?;
    let domain = state.local_domain.to_string();
    Ok(Json(json!({
        "domain": domain,
        "title": site.title,
        "version": compatible_version(),
        "source_url": "https://github.com/seiran-sns/seiran",
        "description": site.description,
        "usage": { "users": { "active_month": 0 } },
        "thumbnail": { "url": site.thumbnail.unwrap_or_else(|| format!("https://{domain}/favicon.ico")) },
        "icon": [],
        "languages": ["ja", "en"],
        "configuration": {
            "urls": { "streaming": format!("wss://{domain}") },
            "vapid": { "public_key": "" },
            "accounts": { "max_featured_tags": 0, "max_pinned_statuses": 0 },
            "statuses": statuses_configuration(),
            "media_attachments": media_configuration(),
            "polls": polls_configuration(),
            "translation": { "enabled": false },
        },
        "registrations": { "enabled": true, "approval_required": false, "message": null },
        "api_versions": { "mastodon": MASTODON_API_VERSION },
        "contact": { "email": "", "account": null },
        "rules": [],
    })))
}

/// GET /api/v1/custom_emojis
pub async fn custom_emojis(State(state): State<AppState>) -> Json<Vec<MastodonEmoji>> {
    Json(
        fetch_public_emojis(&state.db)
            .await
            .into_iter()
            .map(|e| MastodonEmoji {
                shortcode: e.name,
                static_url: e.url.clone(),
                url: e.url,
                visible_in_picker: true,
                category: e.category,
            })
            .collect(),
    )
}

/// GET /api/headers/missing.png — ヘッダー画像未設定のアカウント用（1x1 透明 PNG）。
pub async fn missing_header() -> impl IntoResponse {
    static PNG: OnceLock<Vec<u8>> = OnceLock::new();
    let bytes = PNG.get_or_init(|| {
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
        let mut buf = std::io::Cursor::new(Vec::new());
        // 1x1 の固定画像のエンコードは失敗しない。万一失敗しても空ボディを返すだけ。
        let _ = img.write_to(&mut buf, image::ImageFormat::Png);
        buf.into_inner()
    });
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("image/png")),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            ),
        ],
        bytes.clone(),
    )
}

/// 未実装機能（フィルター・ブックマーク・お知らせ・トレンド等）用。クライアントは起動時や
/// タブを開いたときにこれらを呼び、404 だと画面ごとエラーにするものがあるため空配列を返す。
pub async fn empty_array() -> Json<Vec<Value>> {
    Json(Vec::new())
}

/// 未実装機能のうちオブジェクトを返すもの（`markers`・`preferences`）用。
pub async fn empty_object() -> Json<Value> {
    Json(json!({}))
}

/// GET /api/v1/preferences
pub async fn preferences() -> Json<Value> {
    Json(json!({
        "posting:default:visibility": "public",
        "posting:default:sensitive": false,
        "posting:default:language": null,
        "reading:expand:media": "default",
        "reading:expand:spoilers": false,
    }))
}

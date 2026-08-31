use axum::{
    extract::State,
    http::{header, HeaderValue},
    response::IntoResponse,
};
use serde_json::json;

use crate::AppState;

/// `GET /manifest.webmanifest`
///
/// PWAのWeb App Manifestを動的生成する。サイト名・テーマカラー・アイコンは
/// `site_settings`（管理画面#30で設定）から都度組み立てる。アイコンは
/// `site_icon_sha256` が未設定なら空配列（ホーム画面追加時はブラウザ既定のアイコンになる）。
pub async fn manifest(State(state): State<AppState>) -> impl IntoResponse {
    let settings = state.site_settings.get_all().await.unwrap_or_default();
    let get = |k: &str| settings.get(k).cloned().unwrap_or_default();

    let site_name = {
        let n = get("site_name");
        if n.is_empty() {
            "seiran".to_string()
        } else {
            n
        }
    };
    let theme_color = {
        let c = get("site_color");
        if c.is_empty() {
            "#ffffff".to_string()
        } else {
            c
        }
    };

    let icon_sha256 = get("site_icon_sha256");
    let icons: Vec<serde_json::Value> = if icon_sha256.is_empty() {
        Vec::new()
    } else {
        [192, 512]
            .into_iter()
            .map(|size| {
                json!({
                    "src": format!("/api/site-icon/{icon_sha256}/{size}"),
                    "sizes": format!("{size}x{size}"),
                    "type": "image/png",
                    "purpose": "any",
                })
            })
            .collect()
    };

    let body = json!({
        "name": site_name,
        "short_name": site_name,
        "display": "standalone",
        "start_url": "/",
        "theme_color": theme_color,
        "background_color": theme_color,
        "icons": icons,
    });

    let mut response = axum::Json(body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/manifest+json"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    response
}

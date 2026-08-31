use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};

use crate::AppState;

/// `GET /favicon.ico`
///
/// 管理画面（#30）で設定したサイトアイコンを favicon として返す。
/// ブラウザは JS を介さず直接 `/favicon.ico` を要求するため、SPA では拾えない
/// リンクプレビュー bot 等にも効くようサーバー側でリダイレクトを返す。
/// `site_icon_sha256`（アップロード経由の設定）があれば `/api/site-icon/` の
/// リサイズ済みPNGへ、無ければ従来通り `site_icon_url` へ直接リダイレクトする。
/// どちらも未設定時は 404（ブラウザは既定アイコンにフォールバックする）。
pub async fn favicon(State(state): State<AppState>) -> Response {
    let settings = state.site_settings.get_all().await.unwrap_or_default();
    let sha256 = settings.get("site_icon_sha256").cloned().unwrap_or_default();
    if !sha256.is_empty() {
        return Redirect::temporary(&format!("/api/site-icon/{sha256}/32")).into_response();
    }

    let icon_url = settings.get("site_icon_url").cloned().unwrap_or_default();
    if icon_url.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }
    Redirect::temporary(&icon_url).into_response()
}

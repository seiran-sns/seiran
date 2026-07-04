use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};

use crate::AppState;

/// `GET /favicon.ico`
///
/// 管理画面（#30）で設定したサイトアイコン（`site_icon_url`）を favicon として返す。
/// ブラウザは JS を介さず直接 `/favicon.ico` を要求するため、SPA では拾えない
/// リンクプレビュー bot 等にも効くようサーバー側でリダイレクトを返す。
/// 未設定時は 404（ブラウザは既定アイコンにフォールバックする）。
pub async fn favicon(State(state): State<AppState>) -> Response {
    let icon_url = state
        .site_settings
        .get_all()
        .await
        .ok()
        .and_then(|s| s.get("site_icon_url").cloned())
        .unwrap_or_default();

    if icon_url.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }
    Redirect::temporary(&icon_url).into_response()
}

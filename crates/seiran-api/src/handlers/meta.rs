use axum::{extract::State, response::IntoResponse, Json};
use serde_json::json;

use crate::AppState;

/// Misskey 互換クライアントがサーバー種別判定に使用するエンドポイント。
/// `features.miauth: true` がなければ Aria 等が MiAuth フローに進まない。
pub async fn api_meta(State(state): State<AppState>) -> impl IntoResponse {
    let settings = state.site_settings.get_all().await.unwrap_or_default();
    let get = |k: &str| settings.get(k).cloned().unwrap_or_default();

    let require_email_verification = get("require_email_verification") == "true";

    // サイト外観（#30）。未設定時はデフォルト（name=seiran）。
    let site_name = {
        let n = get("site_name");
        if n.is_empty() { "seiran".to_string() } else { n }
    };

    Json(json!({
        "uri": format!("https://{}", state.local_domain),
        "name": site_name,
        "version": env!("CARGO_PKG_VERSION"),
        "features": {
            "registration": true,
            "miauth": true
        },
        "requireEmailVerification": require_email_verification,
        "siteColor": get("site_color"),
        "siteIconUrl": get("site_icon_url")
    }))
}

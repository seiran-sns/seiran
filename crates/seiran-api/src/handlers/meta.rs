use axum::{extract::State, response::IntoResponse, Json};
use serde_json::json;

use crate::AppState;

/// Misskey 互換クライアントがサーバー種別判定に使用するエンドポイント。
/// `features.miauth: true` がなければ Aria 等が MiAuth フローに進まない。
pub async fn api_meta(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "uri": format!("https://{}", state.local_domain),
        "name": "seiran",
        "version": env!("CARGO_PKG_VERSION"),
        "features": {
            "registration": true,
            "miauth": true
        }
    }))
}

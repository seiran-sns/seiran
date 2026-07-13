use axum::{extract::State, response::IntoResponse, Json};
use serde_json::json;

use crate::handlers::emojis::fetch_public_emojis;
use crate::handlers::notes::BSKY_MAX_TEXT_GRAPHEMES;
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

    // Misskey クライアントの絵文字ピッカー・投稿フォームが参照する標準フィールド。
    // 値は `/api/emojis` および `notes/create` の実際のバリデーションと同じソースを使う。
    let emojis = fetch_public_emojis(&state.db).await;

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
        "siteIconUrl": get("site_icon_url"),
        "emojis": emojis,
        // Bsky 配信時の書記素クラスタ上限（validate_text_length と同じ値）。
        // Fedi のみ配信時は上限が緩む（10,000バイト/3,000書記素）が、Misskey クライアントの
        // 投稿フォームは単一の数値しか扱えないため、より厳しい方（既定の配信先である Bsky 側）を返す。
        "maxNoteTextLength": BSKY_MAX_TEXT_GRAPHEMES,
        // 現状 registration を無効化する設定項目がないため常に false（= 常時登録可能）。
        "disableRegistration": false
    }))
}

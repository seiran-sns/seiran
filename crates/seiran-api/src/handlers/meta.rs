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

    // 自インスタンスのストレージ（R2等、`window.location.origin`とは別サブドメインで
    // 運用されることが多い）はフロントの`/proxy`（SSRF対策・容量上限付き）を経由せず
    // 直接参照させるため、有効なプロバイダーの公開URLをそのまま返す。
    let internal_media_origins: Vec<String> = state
        .storage_providers
        .list_active()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.public_url)
        .collect();

    let require_email_verification = get("require_email_verification") == "true";

    // サイト外観（#30）。未設定時はデフォルト（name=seiran）。
    let site_name = {
        let n = get("site_name");
        if n.is_empty() {
            "seiran".to_string()
        } else {
            n
        }
    };

    // Misskey クライアントの絵文字ピッカー・投稿フォームが参照する標準フィールド。
    // 値は `/api/emojis` および `notes/create` の実際のバリデーションと同じソースを使う。
    let emojis = fetch_public_emojis(&state.db).await;

    // 外部プロキシ未設定時は自インスタンスの `/proxy`（SSRF対策済み、`GET /proxy?url=...`）に
    // フォールバックする（本家Misskeyの慣行）。空文字列のまま返すと、Aria等のクライアントが
    // `{mediaProxyUrl}/image.webp?url=...` という形式でURLを組み立てる際に不正なURLになり、
    // リモートインスタンスアイコン等の画像取得が軒並み失敗する（実機で確認済み）。
    let media_proxy_url = {
        let v = get("media_proxy_url");
        if v.is_empty() {
            format!("https://{}/proxy", state.local_domain)
        } else {
            v
        }
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
        "turnstileSiteKey": get("turnstile_site_key"),
        "siteColor": get("site_color"),
        "siteIconUrl": get("site_icon_url"),
        "mediaProxyUrl": media_proxy_url,
        "internalMediaOrigins": internal_media_origins,
        "emojis": emojis,
        // Bsky 配信時の書記素クラスタ上限（validate_text_length と同じ値）。
        // Fedi のみ配信時は上限が緩む（10,000バイト/3,000書記素）が、Misskey クライアントの
        // 投稿フォームは単一の数値しか扱えないため、より厳しい方（既定の配信先である Bsky 側）を返す。
        "maxNoteTextLength": BSKY_MAX_TEXT_GRAPHEMES,
        // 現状 registration を無効化する設定項目がないため常に false（= 常時登録可能）。
        "disableRegistration": false
    }))
}

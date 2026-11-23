use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::ApiError;
use crate::AppState;

// ─── レスポンス DTO ────────────────────────────────────────────────────────

/// smtp_password はレスポンスに含めず smtp_password_set: bool に置き換える。
#[derive(Serialize)]
pub struct SiteSettingsResponse {
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_username: String,
    pub smtp_password_set: bool,
    pub smtp_from: String,
    pub require_email_verification: String,
    // サイト外観（#30）
    pub site_name: String,
    pub site_color: String,
    pub site_icon_url: String,
    /// アップロード経由で設定した場合の対応 `media_files.sha256`（PWAアイコン配信用）。
    pub site_icon_sha256: String,
    /// アップロード経由で設定した場合の対応 `media_files.id`（文字列）。孤立ファイルGC
    /// （`maintenance::orphaned_media_files`）が参照有無を判定するために見る。
    pub site_icon_media_file_id: String,
    pub media_proxy_url: String,
    // ログイン画面デザイン（#243）
    /// サイト説明テキスト。HTML可・改行保持、サニタイズしない（管理者専用入力のため）。
    pub site_description: String,
    /// 利用規約テキスト。HTML可・改行保持、サニタイズしない（管理者専用入力のため）。
    /// 空文字列なら登録画面は同意チェックボックス自体を表示しない。
    pub terms_of_service_text: String,
    // 「このサイトについて」（`handlers::about`）。いずれも任意。
    /// 運営者名。
    pub operator_name: String,
    /// 運営者の URL（http/https のみ）。
    pub operator_url: String,
    /// 問い合わせ先メールアドレス。
    pub contact_email: String,
    /// ログイン画面背景メディアのURL（画像/動画どちらも可）。
    pub login_bg_url: String,
    /// "image" | "video"。login_bg_url のメディア種別。
    pub login_bg_type: String,
    /// アップロード経由で設定した場合の対応 `media_files.id`（文字列）。用途は
    /// `site_icon_media_file_id` と同じ。
    pub login_bg_media_file_id: String,
    // 認証ブルートフォース対策（#223）
    pub auth_bruteforce_window_minutes: String,
    pub auth_bruteforce_max_variants: String,
    pub auth_ip_block_window_minutes: String,
    pub auth_ip_block_threshold: String,
    pub auth_ip_block_duration_hours: String,
    // Cloudflare Turnstile（#223）: secret_key はレスポンスに含めず設定有無のみ返す
    pub turnstile_site_key: String,
    pub turnstile_secret_key_set: bool,
    // Cloudflare DNS（ハンドル検証 TXT）: api_token はレスポンスに含めず設定有無のみ返す
    pub cloudflare_api_token_set: bool,
    pub cloudflare_zone_id: String,
    // パスワードリセット/確認メールのレート制限（#223）
    pub password_reset_max_active: String,
    // アカウント生成のIPレート制限（#223）
    pub account_creation_ip_window_minutes: String,
    pub account_creation_ip_max: String,
    // ロール別レート制限（#223フォローアップ）
    pub post_rate_limit_window_minutes: String,
    pub post_rate_limit_max_user: String,
    pub post_rate_limit_max_moderator: String,
    pub follow_rate_limit_window_hours: String,
    pub follow_rate_limit_max_user: String,
    pub follow_rate_limit_max_moderator: String,
    pub follow_rate_limit_max_quarantine: String,
    pub list_max_count_user: String,
    pub list_max_count_moderator: String,
    pub list_member_max_user: String,
    pub list_member_max_moderator: String,
    pub search_rate_limit_window_minutes: String,
    pub search_rate_limit_max_user: String,
    pub search_rate_limit_max_moderator: String,
    // URLカード埋め込みプレーヤー（oEmbed discovery）の許可ドメイン（改行区切り、後方一致）
    pub oembed_allowed_domains: String,
    // ユーザー数上限（0 = 無制限）。退会済み・凍結済み・DID転出済みを含むアカウント総数が
    // これに達すると新規登録・転入を拒否する。
    pub max_user_accounts: String,
    /// "true"なら新規登録・転入の受付を停止する。
    pub registration_closed: String,
}

fn build_response(settings: &HashMap<String, String>) -> SiteSettingsResponse {
    SiteSettingsResponse {
        smtp_host: settings.get("smtp_host").cloned().unwrap_or_default(),
        smtp_port: settings.get("smtp_port").cloned().unwrap_or_default(),
        smtp_username: settings.get("smtp_username").cloned().unwrap_or_default(),
        smtp_password_set: settings
            .get("smtp_password")
            .map(|v| !v.is_empty())
            .unwrap_or(false),
        smtp_from: settings.get("smtp_from").cloned().unwrap_or_default(),
        require_email_verification: settings
            .get("require_email_verification")
            .cloned()
            .unwrap_or_else(|| "false".to_string()),
        site_name: settings.get("site_name").cloned().unwrap_or_default(),
        site_color: settings.get("site_color").cloned().unwrap_or_default(),
        site_icon_url: settings.get("site_icon_url").cloned().unwrap_or_default(),
        site_icon_sha256: settings
            .get("site_icon_sha256")
            .cloned()
            .unwrap_or_default(),
        site_icon_media_file_id: settings
            .get("site_icon_media_file_id")
            .cloned()
            .unwrap_or_default(),
        media_proxy_url: settings.get("media_proxy_url").cloned().unwrap_or_default(),
        site_description: settings
            .get("site_description")
            .cloned()
            .unwrap_or_default(),
        terms_of_service_text: settings
            .get("terms_of_service_text")
            .cloned()
            .unwrap_or_default(),
        operator_name: settings.get("operator_name").cloned().unwrap_or_default(),
        operator_url: settings.get("operator_url").cloned().unwrap_or_default(),
        contact_email: settings.get("contact_email").cloned().unwrap_or_default(),
        login_bg_url: settings.get("login_bg_url").cloned().unwrap_or_default(),
        login_bg_type: settings.get("login_bg_type").cloned().unwrap_or_default(),
        login_bg_media_file_id: settings
            .get("login_bg_media_file_id")
            .cloned()
            .unwrap_or_default(),
        auth_bruteforce_window_minutes: settings
            .get("auth_bruteforce_window_minutes")
            .cloned()
            .unwrap_or_else(|| "10".to_string()),
        auth_bruteforce_max_variants: settings
            .get("auth_bruteforce_max_variants")
            .cloned()
            .unwrap_or_else(|| "5".to_string()),
        auth_ip_block_window_minutes: settings
            .get("auth_ip_block_window_minutes")
            .cloned()
            .unwrap_or_else(|| "10".to_string()),
        auth_ip_block_threshold: settings
            .get("auth_ip_block_threshold")
            .cloned()
            .unwrap_or_else(|| "5".to_string()),
        auth_ip_block_duration_hours: settings
            .get("auth_ip_block_duration_hours")
            .cloned()
            .unwrap_or_else(|| "24".to_string()),
        turnstile_site_key: settings
            .get("turnstile_site_key")
            .cloned()
            .unwrap_or_default(),
        turnstile_secret_key_set: settings
            .get("turnstile_secret_key")
            .map(|v| !v.is_empty())
            .unwrap_or(false),
        cloudflare_api_token_set: settings
            .get("cloudflare_api_token")
            .map(|v| !v.is_empty())
            .unwrap_or(false),
        cloudflare_zone_id: settings
            .get("cloudflare_zone_id")
            .cloned()
            .unwrap_or_default(),
        password_reset_max_active: settings
            .get("password_reset_max_active")
            .cloned()
            .unwrap_or_else(|| "10".to_string()),
        account_creation_ip_window_minutes: settings
            .get("account_creation_ip_window_minutes")
            .cloned()
            .unwrap_or_else(|| "60".to_string()),
        account_creation_ip_max: settings
            .get("account_creation_ip_max")
            .cloned()
            .unwrap_or_else(|| "5".to_string()),
        post_rate_limit_window_minutes: settings
            .get("post_rate_limit_window_minutes")
            .cloned()
            .unwrap_or_else(|| "60".to_string()),
        post_rate_limit_max_user: settings
            .get("post_rate_limit_max_user")
            .cloned()
            .unwrap_or_else(|| "30".to_string()),
        post_rate_limit_max_moderator: settings
            .get("post_rate_limit_max_moderator")
            .cloned()
            .unwrap_or_else(|| "100".to_string()),
        follow_rate_limit_window_hours: settings
            .get("follow_rate_limit_window_hours")
            .cloned()
            .unwrap_or_else(|| "24".to_string()),
        follow_rate_limit_max_user: settings
            .get("follow_rate_limit_max_user")
            .cloned()
            .unwrap_or_else(|| "100".to_string()),
        follow_rate_limit_max_moderator: settings
            .get("follow_rate_limit_max_moderator")
            .cloned()
            .unwrap_or_else(|| "300".to_string()),
        follow_rate_limit_max_quarantine: settings
            .get("follow_rate_limit_max_quarantine")
            .cloned()
            .unwrap_or_else(|| "10".to_string()),
        list_max_count_user: settings
            .get("list_max_count_user")
            .cloned()
            .unwrap_or_else(|| "5".to_string()),
        list_max_count_moderator: settings
            .get("list_max_count_moderator")
            .cloned()
            .unwrap_or_else(|| "30".to_string()),
        list_member_max_user: settings
            .get("list_member_max_user")
            .cloned()
            .unwrap_or_else(|| "50".to_string()),
        list_member_max_moderator: settings
            .get("list_member_max_moderator")
            .cloned()
            .unwrap_or_else(|| "300".to_string()),
        search_rate_limit_window_minutes: settings
            .get("search_rate_limit_window_minutes")
            .cloned()
            .unwrap_or_else(|| "60".to_string()),
        search_rate_limit_max_user: settings
            .get("search_rate_limit_max_user")
            .cloned()
            .unwrap_or_else(|| "10".to_string()),
        search_rate_limit_max_moderator: settings
            .get("search_rate_limit_max_moderator")
            .cloned()
            .unwrap_or_else(|| "50".to_string()),
        oembed_allowed_domains: settings
            .get("oembed_allowed_domains")
            .cloned()
            .unwrap_or_default(),
        max_user_accounts: settings
            .get("max_user_accounts")
            .cloned()
            .unwrap_or_else(|| "100".to_string()),
        registration_closed: settings
            .get("registration_closed")
            .cloned()
            .unwrap_or_else(|| "false".to_string()),
    }
}

// ─── リクエスト DTO ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct UpdateSiteSettingsRequest {
    pub smtp_host: Option<String>,
    pub smtp_port: Option<String>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub smtp_from: Option<String>,
    pub require_email_verification: Option<String>,
    pub site_name: Option<String>,
    pub site_color: Option<String>,
    pub site_icon_url: Option<String>,
    pub site_icon_sha256: Option<String>,
    pub site_icon_media_file_id: Option<String>,
    pub media_proxy_url: Option<String>,
    pub site_description: Option<String>,
    pub terms_of_service_text: Option<String>,
    pub operator_name: Option<String>,
    pub operator_url: Option<String>,
    pub contact_email: Option<String>,
    pub login_bg_url: Option<String>,
    pub login_bg_type: Option<String>,
    pub login_bg_media_file_id: Option<String>,
    pub auth_bruteforce_window_minutes: Option<String>,
    pub auth_bruteforce_max_variants: Option<String>,
    pub auth_ip_block_window_minutes: Option<String>,
    pub auth_ip_block_threshold: Option<String>,
    pub auth_ip_block_duration_hours: Option<String>,
    pub turnstile_site_key: Option<String>,
    pub turnstile_secret_key: Option<String>,
    pub cloudflare_api_token: Option<String>,
    pub cloudflare_zone_id: Option<String>,
    pub password_reset_max_active: Option<String>,
    pub account_creation_ip_window_minutes: Option<String>,
    pub account_creation_ip_max: Option<String>,
    pub post_rate_limit_window_minutes: Option<String>,
    pub post_rate_limit_max_user: Option<String>,
    pub post_rate_limit_max_moderator: Option<String>,
    pub follow_rate_limit_window_hours: Option<String>,
    pub follow_rate_limit_max_user: Option<String>,
    pub follow_rate_limit_max_moderator: Option<String>,
    pub follow_rate_limit_max_quarantine: Option<String>,
    pub list_max_count_user: Option<String>,
    pub list_max_count_moderator: Option<String>,
    pub list_member_max_user: Option<String>,
    pub list_member_max_moderator: Option<String>,
    pub search_rate_limit_window_minutes: Option<String>,
    pub search_rate_limit_max_user: Option<String>,
    pub search_rate_limit_max_moderator: Option<String>,
    pub oembed_allowed_domains: Option<String>,
    pub max_user_accounts: Option<String>,
    pub registration_closed: Option<String>,
}

/// `mailto:` リンクにしても壊れない程度のメールアドレスか（`@` の前後が空でなく、空白や
/// `mailto:` の区切りになる文字を含まない）。厳密な検証はしない。
fn is_plausible_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '?' | '&' | '#' | '<' | '>' | '"' | '/'))
        && !domain.contains('@')
}

// ─── ハンドラ ─────────────────────────────────────────────────────────────

/// GET /api/admin/site-settings
pub async fn get_site_settings(
    State(state): State<AppState>,
) -> Result<Json<SiteSettingsResponse>, ApiError> {
    let settings = state
        .site_settings
        .get_all()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(build_response(&settings)))
}

/// PATCH /api/admin/site-settings
pub async fn update_site_settings(
    State(state): State<AppState>,
    Json(req): Json<UpdateSiteSettingsRequest>,
) -> Result<Json<SiteSettingsResponse>, ApiError> {
    if let Some(value) = req
        .media_proxy_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let url = url::Url::parse(value)
            .map_err(|_| ApiError::BadRequest("INVALID_MEDIA_PROXY_URL".to_string()))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(ApiError::BadRequest("INVALID_MEDIA_PROXY_URL".to_string()));
        }
    }

    if let Some(value) = req
        .operator_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        // `/about` でリンクにするので、`javascript:` 等を入れさせない
        let url = url::Url::parse(value)
            .map_err(|_| ApiError::BadRequest("INVALID_OPERATOR_URL".to_string()))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(ApiError::BadRequest("INVALID_OPERATOR_URL".to_string()));
        }
    }
    if let Some(value) = req
        .contact_email
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !is_plausible_email(value) {
            return Err(ApiError::BadRequest("INVALID_CONTACT_EMAIL".to_string()));
        }
    }

    for (field, value) in [
        (
            "auth_bruteforce_window_minutes",
            &req.auth_bruteforce_window_minutes,
        ),
        (
            "auth_bruteforce_max_variants",
            &req.auth_bruteforce_max_variants,
        ),
        (
            "auth_ip_block_window_minutes",
            &req.auth_ip_block_window_minutes,
        ),
        ("auth_ip_block_threshold", &req.auth_ip_block_threshold),
        (
            "auth_ip_block_duration_hours",
            &req.auth_ip_block_duration_hours,
        ),
        ("password_reset_max_active", &req.password_reset_max_active),
        (
            "account_creation_ip_window_minutes",
            &req.account_creation_ip_window_minutes,
        ),
        ("account_creation_ip_max", &req.account_creation_ip_max),
        (
            "post_rate_limit_window_minutes",
            &req.post_rate_limit_window_minutes,
        ),
        ("post_rate_limit_max_user", &req.post_rate_limit_max_user),
        (
            "post_rate_limit_max_moderator",
            &req.post_rate_limit_max_moderator,
        ),
        (
            "follow_rate_limit_window_hours",
            &req.follow_rate_limit_window_hours,
        ),
        (
            "follow_rate_limit_max_user",
            &req.follow_rate_limit_max_user,
        ),
        (
            "follow_rate_limit_max_moderator",
            &req.follow_rate_limit_max_moderator,
        ),
        (
            "follow_rate_limit_max_quarantine",
            &req.follow_rate_limit_max_quarantine,
        ),
        ("list_max_count_user", &req.list_max_count_user),
        ("list_max_count_moderator", &req.list_max_count_moderator),
        ("list_member_max_user", &req.list_member_max_user),
        ("list_member_max_moderator", &req.list_member_max_moderator),
        (
            "search_rate_limit_window_minutes",
            &req.search_rate_limit_window_minutes,
        ),
        (
            "search_rate_limit_max_user",
            &req.search_rate_limit_max_user,
        ),
        (
            "search_rate_limit_max_moderator",
            &req.search_rate_limit_max_moderator,
        ),
        ("site_icon_media_file_id", &req.site_icon_media_file_id),
        ("login_bg_media_file_id", &req.login_bg_media_file_id),
    ] {
        if let Some(v) = value.as_deref().filter(|v| !v.is_empty()) {
            if !matches!(v.parse::<i64>(), Ok(n) if n > 0) {
                return Err(ApiError::BadRequest(format!(
                    "INVALID_{}",
                    field.to_uppercase()
                )));
            }
        }
    }

    // 0は無制限。他の数値設定と違い0を許す。
    if let Some(v) = req.max_user_accounts.as_deref().filter(|v| !v.is_empty()) {
        if !matches!(v.parse::<i64>(), Ok(n) if n >= 0) {
            return Err(ApiError::BadRequest(
                "INVALID_MAX_USER_ACCOUNTS".to_string(),
            ));
        }
    }

    let pairs: Vec<(&str, String)> = [
        req.smtp_host
            .as_deref()
            .map(|v| ("smtp_host", v.to_string())),
        req.smtp_port
            .as_deref()
            .map(|v| ("smtp_port", v.to_string())),
        req.smtp_username
            .as_deref()
            .map(|v| ("smtp_username", v.to_string())),
        req.smtp_password
            .as_deref()
            .map(|v| ("smtp_password", v.to_string())),
        req.smtp_from
            .as_deref()
            .map(|v| ("smtp_from", v.to_string())),
        req.require_email_verification
            .as_deref()
            .map(|v| ("require_email_verification", v.to_string())),
        req.site_name
            .as_deref()
            .map(|v| ("site_name", v.to_string())),
        req.site_color
            .as_deref()
            .map(|v| ("site_color", v.to_string())),
        req.site_icon_url
            .as_deref()
            .map(|v| ("site_icon_url", v.to_string())),
        req.site_icon_sha256
            .as_deref()
            .map(|v| ("site_icon_sha256", v.to_string())),
        req.site_icon_media_file_id
            .as_deref()
            .map(|v| ("site_icon_media_file_id", v.to_string())),
        req.media_proxy_url
            .as_deref()
            .map(|v| ("media_proxy_url", v.trim_end_matches('/').to_string())),
        req.site_description
            .as_deref()
            .map(|v| ("site_description", v.to_string())),
        req.terms_of_service_text
            .as_deref()
            .map(|v| ("terms_of_service_text", v.to_string())),
        req.operator_name
            .as_deref()
            .map(|v| ("operator_name", v.trim().to_string())),
        req.operator_url
            .as_deref()
            .map(|v| ("operator_url", v.trim().to_string())),
        req.contact_email
            .as_deref()
            .map(|v| ("contact_email", v.trim().to_string())),
        req.login_bg_url
            .as_deref()
            .map(|v| ("login_bg_url", v.to_string())),
        req.login_bg_type
            .as_deref()
            .map(|v| ("login_bg_type", v.to_string())),
        req.login_bg_media_file_id
            .as_deref()
            .map(|v| ("login_bg_media_file_id", v.to_string())),
        req.auth_bruteforce_window_minutes
            .as_deref()
            .map(|v| ("auth_bruteforce_window_minutes", v.to_string())),
        req.auth_bruteforce_max_variants
            .as_deref()
            .map(|v| ("auth_bruteforce_max_variants", v.to_string())),
        req.auth_ip_block_window_minutes
            .as_deref()
            .map(|v| ("auth_ip_block_window_minutes", v.to_string())),
        req.auth_ip_block_threshold
            .as_deref()
            .map(|v| ("auth_ip_block_threshold", v.to_string())),
        req.auth_ip_block_duration_hours
            .as_deref()
            .map(|v| ("auth_ip_block_duration_hours", v.to_string())),
        req.turnstile_site_key
            .as_deref()
            .map(|v| ("turnstile_site_key", v.to_string())),
        req.turnstile_secret_key
            .as_deref()
            .map(|v| ("turnstile_secret_key", v.to_string())),
        req.cloudflare_api_token
            .as_deref()
            .map(|v| ("cloudflare_api_token", v.trim().to_string())),
        req.cloudflare_zone_id
            .as_deref()
            .map(|v| ("cloudflare_zone_id", v.trim().to_string())),
        req.password_reset_max_active
            .as_deref()
            .map(|v| ("password_reset_max_active", v.to_string())),
        req.account_creation_ip_window_minutes
            .as_deref()
            .map(|v| ("account_creation_ip_window_minutes", v.to_string())),
        req.account_creation_ip_max
            .as_deref()
            .map(|v| ("account_creation_ip_max", v.to_string())),
        req.post_rate_limit_window_minutes
            .as_deref()
            .map(|v| ("post_rate_limit_window_minutes", v.to_string())),
        req.post_rate_limit_max_user
            .as_deref()
            .map(|v| ("post_rate_limit_max_user", v.to_string())),
        req.post_rate_limit_max_moderator
            .as_deref()
            .map(|v| ("post_rate_limit_max_moderator", v.to_string())),
        req.follow_rate_limit_window_hours
            .as_deref()
            .map(|v| ("follow_rate_limit_window_hours", v.to_string())),
        req.follow_rate_limit_max_user
            .as_deref()
            .map(|v| ("follow_rate_limit_max_user", v.to_string())),
        req.follow_rate_limit_max_moderator
            .as_deref()
            .map(|v| ("follow_rate_limit_max_moderator", v.to_string())),
        req.follow_rate_limit_max_quarantine
            .as_deref()
            .map(|v| ("follow_rate_limit_max_quarantine", v.to_string())),
        req.list_max_count_user
            .as_deref()
            .map(|v| ("list_max_count_user", v.to_string())),
        req.list_max_count_moderator
            .as_deref()
            .map(|v| ("list_max_count_moderator", v.to_string())),
        req.list_member_max_user
            .as_deref()
            .map(|v| ("list_member_max_user", v.to_string())),
        req.list_member_max_moderator
            .as_deref()
            .map(|v| ("list_member_max_moderator", v.to_string())),
        req.search_rate_limit_window_minutes
            .as_deref()
            .map(|v| ("search_rate_limit_window_minutes", v.to_string())),
        req.search_rate_limit_max_user
            .as_deref()
            .map(|v| ("search_rate_limit_max_user", v.to_string())),
        req.search_rate_limit_max_moderator
            .as_deref()
            .map(|v| ("search_rate_limit_max_moderator", v.to_string())),
        req.oembed_allowed_domains
            .as_deref()
            .map(|v| ("oembed_allowed_domains", v.to_string())),
        req.max_user_accounts
            .as_deref()
            .map(|v| ("max_user_accounts", v.to_string())),
        req.registration_closed
            .as_deref()
            .map(|v| ("registration_closed", v.to_string())),
    ]
    .into_iter()
    .flatten()
    .collect();

    for (key, value) in &pairs {
        state
            .site_settings
            .set(key, value)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }

    let settings = state
        .site_settings
        .get_all()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(build_response(&settings)))
}

// ─── SMTP の接続確認・テスト送信 ───────────────────────────────────────────

/// 管理画面のフォームに入っている（保存前の）SMTP 設定。保存してからでないと試せないと、
/// 誤った設定を一度保存してメール確認・パスワードリセットを壊してしまうため、
/// 入力中の値で試せるようにする。省略した項目と空のパスワードは保存済みの値を使う。
#[derive(Deserialize, Default)]
pub struct SmtpTestRequest {
    pub smtp_host: Option<String>,
    pub smtp_port: Option<String>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub smtp_from: Option<String>,
    /// テスト送信の宛先。接続確認では使わない。
    pub to: Option<String>,
    /// テスト送信の言語（管理画面の表示言語）。無い・非対応なら `Accept-Language` から決める。
    pub language: Option<String>,
}

/// SMTP サーバーの応答はそのまま管理者に見せたいので、失敗も 200 で返し、
/// `code`（画面で翻訳する種別）と `detail`（lettre のエラー文・未設定の項目名）を載せる。
#[derive(Serialize)]
pub struct SmtpTestResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl SmtpTestResponse {
    fn from_result(result: Result<(), crate::mailer::MailError>) -> Self {
        use crate::mailer::MailError;
        match result {
            Ok(()) => Self {
                ok: true,
                code: None,
                detail: None,
            },
            Err(e) => {
                let (code, detail) = match e {
                    MailError::Config(field) => ("SMTP_NOT_CONFIGURED", field.to_string()),
                    MailError::Address(e) => ("SMTP_INVALID_ADDRESS", e.to_string()),
                    other
                    @ (MailError::Build(_) | MailError::Send(_) | MailError::NotConnected) => {
                        ("SMTP_FAILED", other.to_string())
                    }
                };
                Self {
                    ok: false,
                    code: Some(code),
                    detail: Some(detail),
                }
            }
        }
    }
}

/// 保存済みの設定に、リクエストで渡された SMTP 項目を重ねる。
fn merge_smtp_overrides(
    mut settings: HashMap<String, String>,
    req: &SmtpTestRequest,
) -> HashMap<String, String> {
    let password = req.smtp_password.as_ref().filter(|v| !v.is_empty());
    for (key, value) in [
        ("smtp_host", req.smtp_host.as_ref()),
        ("smtp_port", req.smtp_port.as_ref()),
        ("smtp_username", req.smtp_username.as_ref()),
        ("smtp_password", password),
        ("smtp_from", req.smtp_from.as_ref()),
    ] {
        if let Some(v) = value {
            settings.insert(key.to_string(), v.trim().to_string());
        }
    }
    settings
}

async fn smtp_settings_for_test(
    state: &AppState,
    req: &SmtpTestRequest,
) -> Result<HashMap<String, String>, ApiError> {
    let saved = state
        .site_settings
        .get_all()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(merge_smtp_overrides(saved, req))
}

/// POST /api/admin/site-settings/smtp-test
pub async fn test_smtp_connection(
    State(state): State<AppState>,
    Json(req): Json<SmtpTestRequest>,
) -> Result<Json<SmtpTestResponse>, ApiError> {
    let settings = smtp_settings_for_test(&state, &req).await?;
    let result = crate::mailer::test_smtp_connection(&settings).await;
    Ok(Json(SmtpTestResponse::from_result(result)))
}

/// POST /api/admin/site-settings/smtp-test-email
pub async fn send_smtp_test_email(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SmtpTestRequest>,
) -> Result<Json<SmtpTestResponse>, ApiError> {
    let to = req
        .to
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::BadRequest("SMTP_TEST_RECIPIENT_REQUIRED".to_string()))?
        .to_string();
    let settings = smtp_settings_for_test(&state, &req).await?;
    let language = crate::i18n::request_language(req.language.as_deref(), &headers);
    let result = crate::mailer::send_test_email(&settings, language, &to).await;
    Ok(Json(SmtpTestResponse::from_result(result)))
}

// ─── Turnstile の動作確認 ─────────────────────────────────────────────────

/// 管理画面のウィジェットで得たトークンと、入力中の（保存前の）Secret Key。鍵を誤って
/// 保存すると登録が `TURNSTILE_FAILED` で全滅するので、保存前に試せるようにする。
/// Secret Key を省略・空にすると保存済みの値を使う。
#[derive(Deserialize)]
pub struct TurnstileTestRequest {
    pub token: String,
    pub turnstile_secret_key: Option<String>,
}

/// SMTP の動作確認と同じく失敗も 200 で返す。`detail` は Siteverify の `error-codes`
/// （`invalid-input-secret` 等）か通信エラー文、`hostname` はウィジェットを解いたホスト名。
#[derive(Serialize)]
pub struct TurnstileTestResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

impl TurnstileTestResponse {
    fn failure(code: &'static str, detail: Option<String>) -> Self {
        Self {
            ok: false,
            code: Some(code),
            detail,
            hostname: None,
        }
    }
}

/// POST /api/admin/site-settings/turnstile-test
pub async fn test_turnstile(
    State(state): State<AppState>,
    ip: crate::middleware::ClientIp,
    Json(req): Json<TurnstileTestRequest>,
) -> Result<Json<TurnstileTestResponse>, ApiError> {
    let secret = match req
        .turnstile_secret_key
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        Some(v) => v.to_string(),
        None => state
            .site_settings
            .get("turnstile_secret_key")
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .unwrap_or_default()
            .trim()
            .to_string(),
    };
    if secret.is_empty() || req.token.trim().is_empty() {
        return Ok(Json(TurnstileTestResponse::failure(
            "TURNSTILE_NOT_CONFIGURED",
            None,
        )));
    }
    let response =
        match crate::handlers::auth::siteverify(&state, &secret, &req.token, ip.0.as_deref()).await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(Json(TurnstileTestResponse::failure(
                    "TURNSTILE_UNREACHABLE",
                    Some(e),
                )))
            }
        };
    if !response.success {
        let detail = (!response.error_codes.is_empty()).then(|| response.error_codes.join(", "));
        return Ok(Json(TurnstileTestResponse::failure(
            "TURNSTILE_FAILED",
            detail,
        )));
    }
    Ok(Json(TurnstileTestResponse {
        ok: true,
        code: None,
        detail: None,
        hostname: response.hostname,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plausible_email() {
        assert!(is_plausible_email("admin@example.com"));
        assert!(is_plausible_email("a.b+c@sub.example.jp"));
        for bad in [
            "admin",
            "@example.com",
            "admin@",
            "admin@localhost",
            "a b@example.com",
            "a@example.com?subject=x",
            "a@b@example.com",
            "a@.example.com",
        ] {
            assert!(!is_plausible_email(bad), "{bad}");
        }
    }

    #[test]
    fn overrides_replace_saved_values_but_empty_password_keeps_saved() {
        let saved = HashMap::from([
            ("smtp_host".to_string(), "old.example".to_string()),
            ("smtp_password".to_string(), "saved-secret".to_string()),
            ("smtp_from".to_string(), "a@example.com".to_string()),
        ]);
        let req = SmtpTestRequest {
            smtp_host: Some(" new.example ".to_string()),
            smtp_password: Some(String::new()),
            ..Default::default()
        };
        let merged = merge_smtp_overrides(saved, &req);
        assert_eq!(merged["smtp_host"], "new.example");
        assert_eq!(merged["smtp_password"], "saved-secret");
        assert_eq!(merged["smtp_from"], "a@example.com");
    }

    #[test]
    fn missing_host_reports_not_configured() {
        let res = SmtpTestResponse::from_result(Err(crate::mailer::MailError::Config("smtp_host")));
        assert!(!res.ok);
        assert_eq!(res.code, Some("SMTP_NOT_CONFIGURED"));
        assert_eq!(res.detail.as_deref(), Some("smtp_host"));
    }
}

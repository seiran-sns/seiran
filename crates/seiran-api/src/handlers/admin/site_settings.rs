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
    pub media_proxy_url: String,
    // 認証ブルートフォース対策（#223）
    pub auth_bruteforce_window_minutes: String,
    pub auth_bruteforce_max_variants: String,
    pub auth_ip_block_window_minutes: String,
    pub auth_ip_block_threshold: String,
    pub auth_ip_block_duration_hours: String,
    // Cloudflare Turnstile（#223）: secret_key はレスポンスに含めず設定有無のみ返す
    pub turnstile_site_key: String,
    pub turnstile_secret_key_set: bool,
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
    pub list_max_count_user: String,
    pub list_max_count_moderator: String,
    pub list_member_max_user: String,
    pub list_member_max_moderator: String,
    pub search_rate_limit_window_minutes: String,
    pub search_rate_limit_max_user: String,
    pub search_rate_limit_max_moderator: String,
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
        media_proxy_url: settings.get("media_proxy_url").cloned().unwrap_or_default(),
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
    pub media_proxy_url: Option<String>,
    pub auth_bruteforce_window_minutes: Option<String>,
    pub auth_bruteforce_max_variants: Option<String>,
    pub auth_ip_block_window_minutes: Option<String>,
    pub auth_ip_block_threshold: Option<String>,
    pub auth_ip_block_duration_hours: Option<String>,
    pub turnstile_site_key: Option<String>,
    pub turnstile_secret_key: Option<String>,
    pub password_reset_max_active: Option<String>,
    pub account_creation_ip_window_minutes: Option<String>,
    pub account_creation_ip_max: Option<String>,
    pub post_rate_limit_window_minutes: Option<String>,
    pub post_rate_limit_max_user: Option<String>,
    pub post_rate_limit_max_moderator: Option<String>,
    pub follow_rate_limit_window_hours: Option<String>,
    pub follow_rate_limit_max_user: Option<String>,
    pub follow_rate_limit_max_moderator: Option<String>,
    pub list_max_count_user: Option<String>,
    pub list_max_count_moderator: Option<String>,
    pub list_member_max_user: Option<String>,
    pub list_member_max_moderator: Option<String>,
    pub search_rate_limit_window_minutes: Option<String>,
    pub search_rate_limit_max_user: Option<String>,
    pub search_rate_limit_max_moderator: Option<String>,
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
    ] {
        if let Some(v) = value.as_deref() {
            if !matches!(v.parse::<i64>(), Ok(n) if n > 0) {
                return Err(ApiError::BadRequest(format!(
                    "INVALID_{}",
                    field.to_uppercase()
                )));
            }
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
        req.media_proxy_url
            .as_deref()
            .map(|v| ("media_proxy_url", v.trim_end_matches('/').to_string())),
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

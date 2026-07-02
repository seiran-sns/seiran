use axum::{
    extract::State,
    http::HeaderMap,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::AppState;
use crate::error::ApiError;
use crate::middleware::require_admin;

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
}

// ─── ハンドラ ─────────────────────────────────────────────────────────────

/// GET /api/admin/site-settings
pub async fn get_site_settings(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<SiteSettingsResponse>, ApiError> {
    require_admin(&headers, &state.local_auth, state.users.as_ref()).await?;

    let settings = state
        .site_settings
        .get_all()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(build_response(&settings)))
}

/// PATCH /api/admin/site-settings
pub async fn update_site_settings(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<UpdateSiteSettingsRequest>,
) -> Result<Json<SiteSettingsResponse>, ApiError> {
    require_admin(&headers, &state.local_auth, state.users.as_ref()).await?;

    let pairs: Vec<(&str, String)> = [
        req.smtp_host.as_deref().map(|v| ("smtp_host", v.to_string())),
        req.smtp_port.as_deref().map(|v| ("smtp_port", v.to_string())),
        req.smtp_username.as_deref().map(|v| ("smtp_username", v.to_string())),
        req.smtp_password.as_deref().map(|v| ("smtp_password", v.to_string())),
        req.smtp_from.as_deref().map(|v| ("smtp_from", v.to_string())),
        req.require_email_verification.as_deref().map(|v| ("require_email_verification", v.to_string())),
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

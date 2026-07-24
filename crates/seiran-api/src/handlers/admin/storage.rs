use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use seiran_common::repository::{
    CreateStorageProvider, StorageProvider, StorageProviderError, UpdateStorageProvider,
};

use crate::AppState;
use crate::error::ApiError;
use crate::middleware::require_admin;

// ─── レスポンス DTO ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct StorageProviderResponse {
    pub id: i64,
    pub name: String,
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    /// secret_key は平文では返さない
    pub secret_key_set: bool,
    pub public_url: String,
    pub capacity_mb: Option<i64>,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
}

impl From<StorageProvider> for StorageProviderResponse {
    fn from(p: StorageProvider) -> Self {
        Self {
            id: p.id,
            name: p.name,
            endpoint: p.endpoint,
            bucket: p.bucket,
            region: p.region,
            access_key: p.access_key,
            secret_key_set: !p.secret_key.is_empty(),
            public_url: p.public_url,
            capacity_mb: p.capacity_mb,
            is_active: p.is_active,
            created_at: p.created_at,
        }
    }
}

// ─── リクエスト DTO ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateStorageProviderRequest {
    pub name: String,
    pub endpoint: String,
    pub bucket: String,
    #[serde(default = "default_region")]
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub public_url: String,
    pub capacity_mb: Option<i64>,
}

fn default_region() -> String {
    "auto".to_string()
}

#[derive(Deserialize)]
pub struct UpdateStorageProviderRequest {
    pub name: Option<String>,
    pub endpoint: Option<String>,
    pub bucket: Option<String>,
    pub region: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub public_url: Option<String>,
    /// `null` を明示すると capacity_mb が NULL になる
    pub capacity_mb: Option<Option<i64>>,
    pub is_active: Option<bool>,
}

// ─── エラー変換 ───────────────────────────────────────────────────────────

fn sp_err(e: StorageProviderError) -> ApiError {
    match e {
        StorageProviderError::HasMediaFiles => ApiError::Conflict("HAS_MEDIA_FILES"),
        StorageProviderError::Db(e) => ApiError::Internal(e.to_string()),
        StorageProviderError::Crypto(e) => ApiError::Internal(e.to_string()),
    }
}

// ─── ハンドラ ─────────────────────────────────────────────────────────────

/// GET /api/admin/storage-providers
pub async fn list_storage_providers(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Vec<StorageProviderResponse>>, ApiError> {
    require_admin(&headers, &state.local_auth, state.app_tokens.as_ref(), state.users.as_ref()).await?;
    let providers = state.storage_providers.list_all().await.map_err(sp_err)?;
    Ok(Json(providers.into_iter().map(Into::into).collect()))
}

/// POST /api/admin/storage-providers
pub async fn create_storage_provider(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateStorageProviderRequest>,
) -> Result<Json<StorageProviderResponse>, ApiError> {
    require_admin(&headers, &state.local_auth, state.app_tokens.as_ref(), state.users.as_ref()).await?;
    if req.name.is_empty() || req.endpoint.is_empty() || req.bucket.is_empty()
        || req.access_key.is_empty() || req.secret_key.is_empty() || req.public_url.is_empty()
    {
        return Err(ApiError::BadRequest("INVALID_INPUT".into()));
    }
    let provider = state
        .storage_providers
        .insert(CreateStorageProvider {
            name: req.name,
            endpoint: req.endpoint,
            bucket: req.bucket,
            region: req.region,
            access_key: req.access_key,
            secret_key: req.secret_key,
            public_url: req.public_url,
            capacity_mb: req.capacity_mb,
        })
        .await
        .map_err(sp_err)?;
    Ok(Json(provider.into()))
}

/// PATCH /api/admin/storage-providers/:id
pub async fn update_storage_provider(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateStorageProviderRequest>,
) -> Result<Json<StorageProviderResponse>, ApiError> {
    require_admin(&headers, &state.local_auth, state.app_tokens.as_ref(), state.users.as_ref()).await?;
    let provider = state
        .storage_providers
        .update(
            id,
            UpdateStorageProvider {
                name: req.name,
                endpoint: req.endpoint,
                bucket: req.bucket,
                region: req.region,
                access_key: req.access_key,
                secret_key: req.secret_key,
                public_url: req.public_url,
                capacity_mb: req.capacity_mb,
                is_active: req.is_active,
            },
        )
        .await
        .map_err(sp_err)?
        .ok_or(ApiError::NotFound("STORAGE_PROVIDER_NOT_FOUND"))?;
    Ok(Json(provider.into()))
}

/// DELETE /api/admin/storage-providers/:id
pub async fn delete_storage_provider(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<axum::http::StatusCode, ApiError> {
    require_admin(&headers, &state.local_auth, state.app_tokens.as_ref(), state.users.as_ref()).await?;
    state.storage_providers.delete(id).await.map_err(sp_err)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

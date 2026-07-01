use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::crypto::{decrypt, encrypt, CryptoError};

#[derive(Debug, Clone)]
pub struct StorageProvider {
    pub id: i64,
    pub name: String,
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    /// 復号済みの plaintext secret_key
    pub secret_key: String,
    pub public_url: String,
    pub capacity_mb: Option<i64>,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
}

pub struct CreateStorageProvider {
    pub name: String,
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub public_url: String,
    pub capacity_mb: Option<i64>,
}

pub struct UpdateStorageProvider {
    pub name: Option<String>,
    pub endpoint: Option<String>,
    pub bucket: Option<String>,
    pub region: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub public_url: Option<String>,
    pub capacity_mb: Option<Option<i64>>,
    pub is_active: Option<bool>,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageProviderError {
    #[error("DB エラー: {0}")]
    Db(#[from] sqlx::Error),
    #[error("暗号化エラー: {0}")]
    Crypto(#[from] CryptoError),
    #[error("参照中のメディアファイルが存在するため削除できません")]
    HasMediaFiles,
}

#[async_trait]
pub trait StorageProviderRepository: Send + Sync {
    async fn list_all(&self) -> Result<Vec<StorageProvider>, StorageProviderError>;
    async fn list_active(&self) -> Result<Vec<StorageProvider>, StorageProviderError>;
    async fn find_by_id(&self, id: i64) -> Result<Option<StorageProvider>, StorageProviderError>;
    async fn insert(&self, req: CreateStorageProvider) -> Result<StorageProvider, StorageProviderError>;
    async fn update(&self, id: i64, req: UpdateStorageProvider) -> Result<Option<StorageProvider>, StorageProviderError>;
    async fn delete(&self, id: i64) -> Result<(), StorageProviderError>;
    /// プロバイダーに保存済みのバイト数合計。capacity チェックに使う。
    async fn get_used_bytes(&self, provider_id: i64) -> Result<i64, StorageProviderError>;
}

pub struct PgStorageProviderRepository {
    pool: PgPool,
    encryption_key: Vec<u8>,
}

impl PgStorageProviderRepository {
    pub fn new(pool: PgPool, encryption_key: Vec<u8>) -> Self {
        Self { pool, encryption_key }
    }

    fn decrypt_row(&self, row: &StorageProviderRow) -> Result<StorageProvider, StorageProviderError> {
        let secret_key = decrypt(&row.secret_key_enc, &self.encryption_key)?;
        let secret_key = String::from_utf8(secret_key)
            .map_err(|_| CryptoError::InvalidData)?;
        Ok(StorageProvider {
            id: row.id,
            name: row.name.clone(),
            endpoint: row.endpoint.clone(),
            bucket: row.bucket.clone(),
            region: row.region.clone(),
            access_key: row.access_key.clone(),
            secret_key,
            public_url: row.public_url.clone(),
            capacity_mb: row.capacity_mb,
            is_active: row.is_active,
            created_at: row.created_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct StorageProviderRow {
    id: i64,
    name: String,
    endpoint: String,
    bucket: String,
    region: String,
    access_key: String,
    secret_key_enc: String,
    public_url: String,
    capacity_mb: Option<i64>,
    is_active: bool,
    created_at: DateTime<Utc>,
}

#[async_trait]
impl StorageProviderRepository for PgStorageProviderRepository {
    async fn list_all(&self) -> Result<Vec<StorageProvider>, StorageProviderError> {
        let rows = sqlx::query_as::<_, StorageProviderRow>(
            "SELECT id, name, endpoint, bucket, region, access_key,
                    secret_key AS secret_key_enc, public_url, capacity_mb, is_active, created_at
             FROM storage_providers
             ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(|r| self.decrypt_row(r)).collect()
    }

    async fn list_active(&self) -> Result<Vec<StorageProvider>, StorageProviderError> {
        let rows = sqlx::query_as::<_, StorageProviderRow>(
            "SELECT id, name, endpoint, bucket, region, access_key,
                    secret_key AS secret_key_enc, public_url, capacity_mb, is_active, created_at
             FROM storage_providers
             WHERE is_active = true
             ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(|r| self.decrypt_row(r)).collect()
    }

    async fn find_by_id(&self, id: i64) -> Result<Option<StorageProvider>, StorageProviderError> {
        let row = sqlx::query_as::<_, StorageProviderRow>(
            "SELECT id, name, endpoint, bucket, region, access_key,
                    secret_key AS secret_key_enc, public_url, capacity_mb, is_active, created_at
             FROM storage_providers WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(|r| self.decrypt_row(r)).transpose()
    }

    async fn insert(&self, req: CreateStorageProvider) -> Result<StorageProvider, StorageProviderError> {
        let secret_key_enc = encrypt(req.secret_key.as_bytes(), &self.encryption_key)?;
        let row = sqlx::query_as::<_, StorageProviderRow>(
            "INSERT INTO storage_providers
                (name, endpoint, bucket, region, access_key, secret_key, public_url, capacity_mb)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id, name, endpoint, bucket, region, access_key,
                       secret_key AS secret_key_enc, public_url, capacity_mb, is_active, created_at",
        )
        .bind(&req.name)
        .bind(&req.endpoint)
        .bind(&req.bucket)
        .bind(&req.region)
        .bind(&req.access_key)
        .bind(&secret_key_enc)
        .bind(&req.public_url)
        .bind(req.capacity_mb)
        .fetch_one(&self.pool)
        .await?;
        self.decrypt_row(&row)
    }

    async fn update(&self, id: i64, req: UpdateStorageProvider) -> Result<Option<StorageProvider>, StorageProviderError> {
        // 現在の値を取得
        let current = match self.find_by_id(id).await? {
            Some(p) => p,
            None => return Ok(None),
        };

        let new_secret_key = req.secret_key.as_deref().unwrap_or(&current.secret_key);
        let secret_key_enc = encrypt(new_secret_key.as_bytes(), &self.encryption_key)?;

        let row = sqlx::query_as::<_, StorageProviderRow>(
            "UPDATE storage_providers SET
                name        = $2,
                endpoint    = $3,
                bucket      = $4,
                region      = $5,
                access_key  = $6,
                secret_key  = $7,
                public_url  = $8,
                capacity_mb = $9,
                is_active   = $10
             WHERE id = $1
             RETURNING id, name, endpoint, bucket, region, access_key,
                       secret_key AS secret_key_enc, public_url, capacity_mb, is_active, created_at",
        )
        .bind(id)
        .bind(req.name.as_deref().unwrap_or(&current.name))
        .bind(req.endpoint.as_deref().unwrap_or(&current.endpoint))
        .bind(req.bucket.as_deref().unwrap_or(&current.bucket))
        .bind(req.region.as_deref().unwrap_or(&current.region))
        .bind(req.access_key.as_deref().unwrap_or(&current.access_key))
        .bind(&secret_key_enc)
        .bind(req.public_url.as_deref().unwrap_or(&current.public_url))
        .bind(req.capacity_mb.unwrap_or(current.capacity_mb))
        .bind(req.is_active.unwrap_or(current.is_active))
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(|r| self.decrypt_row(r)).transpose()
    }

    async fn delete(&self, id: i64) -> Result<(), StorageProviderError> {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM media_files WHERE storage_provider_id = $1"
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        if count.0 > 0 {
            return Err(StorageProviderError::HasMediaFiles);
        }
        sqlx::query("DELETE FROM storage_providers WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_used_bytes(&self, provider_id: i64) -> Result<i64, StorageProviderError> {
        let row: (Option<i64>,) = sqlx::query_as(
            "SELECT SUM(size) FROM media_files WHERE storage_provider_id = $1"
        )
        .bind(provider_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0.unwrap_or(0))
    }
}

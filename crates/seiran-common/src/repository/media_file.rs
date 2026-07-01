use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MediaFile {
    pub id: i64,
    pub storage_provider_id: i64,
    pub sha256: String,
    pub blurhash: String,
    pub size: i64,
    pub width: i32,
    pub height: i32,
    pub mime_type: String,
    pub storage_key: String,
    pub uploaded_by_actor_id: Option<i64>,
    pub created_at: DateTime<Utc>,
}

pub struct CreateMediaFile {
    pub id: i64,
    pub storage_provider_id: i64,
    pub sha256: String,
    pub blurhash: String,
    pub size: i64,
    pub width: i32,
    pub height: i32,
    pub mime_type: String,
    pub storage_key: String,
    pub uploaded_by_actor_id: Option<i64>,
}

#[derive(Debug, thiserror::Error)]
pub enum MediaFileError {
    #[error("DB エラー: {0}")]
    Db(#[from] sqlx::Error),
}

const SELECT_COLS: &str =
    "id, storage_provider_id, sha256, blurhash, size, width, height, mime_type, storage_key, uploaded_by_actor_id, created_at";

#[async_trait]
pub trait MediaFileRepository: Send + Sync {
    /// SHA-256 と blurhash が一致するファイルを返す（重複排除用）。
    async fn find_by_sha256_and_blurhash(
        &self,
        sha256: &str,
        blurhash: &str,
    ) -> Result<Option<MediaFile>, MediaFileError>;

    async fn find_by_id(&self, id: i64) -> Result<Option<MediaFile>, MediaFileError>;

    async fn insert(&self, req: CreateMediaFile) -> Result<MediaFile, MediaFileError>;

    async fn delete_by_id(&self, id: i64) -> Result<(), MediaFileError>;
}

pub struct PgMediaFileRepository {
    pool: PgPool,
}

impl PgMediaFileRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MediaFileRepository for PgMediaFileRepository {
    async fn find_by_sha256_and_blurhash(
        &self,
        sha256: &str,
        blurhash: &str,
    ) -> Result<Option<MediaFile>, MediaFileError> {
        let row = sqlx::query_as::<_, MediaFile>(&format!(
            "SELECT {SELECT_COLS} FROM media_files WHERE sha256 = $1 AND blurhash = $2 LIMIT 1"
        ))
        .bind(sha256)
        .bind(blurhash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn find_by_id(&self, id: i64) -> Result<Option<MediaFile>, MediaFileError> {
        let row = sqlx::query_as::<_, MediaFile>(&format!(
            "SELECT {SELECT_COLS} FROM media_files WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn insert(&self, req: CreateMediaFile) -> Result<MediaFile, MediaFileError> {
        let row = sqlx::query_as::<_, MediaFile>(&format!(
            "INSERT INTO media_files \
             (id, storage_provider_id, sha256, blurhash, size, width, height, mime_type, storage_key, uploaded_by_actor_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) \
             RETURNING {SELECT_COLS}"
        ))
        .bind(req.id)
        .bind(req.storage_provider_id)
        .bind(req.sha256)
        .bind(req.blurhash)
        .bind(req.size)
        .bind(req.width)
        .bind(req.height)
        .bind(req.mime_type)
        .bind(req.storage_key)
        .bind(req.uploaded_by_actor_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    async fn delete_by_id(&self, id: i64) -> Result<(), MediaFileError> {
        sqlx::query("DELETE FROM media_files WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

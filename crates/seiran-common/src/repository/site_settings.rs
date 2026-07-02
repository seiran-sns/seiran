use async_trait::async_trait;
use std::collections::HashMap;
use sqlx::PgPool;

#[async_trait]
pub trait SiteSettingsRepository: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>, sqlx::Error>;
    async fn set(&self, key: &str, value: &str) -> Result<(), sqlx::Error>;
    async fn get_all(&self) -> Result<HashMap<String, String>, sqlx::Error>;
}

pub struct PgSiteSettingsRepository {
    pool: PgPool,
}

impl PgSiteSettingsRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SiteSettingsRepository for PgSiteSettingsRepository {
    async fn get(&self, key: &str) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT value FROM site_settings WHERE key = $1",
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(v,)| v))
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO site_settings (key, value, updated_at)
             VALUES ($1, $2, NOW())
             ON CONFLICT (key) DO UPDATE
               SET value = EXCLUDED.value,
                   updated_at = NOW()",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_all(&self) -> Result<HashMap<String, String>, sqlx::Error> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT key, value FROM site_settings",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().collect())
    }
}

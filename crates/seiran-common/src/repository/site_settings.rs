use async_trait::async_trait;
use sqlx::PgPool;
use std::collections::HashMap;

use crate::edit_diff::EditConflict;

/// `update_with` に渡す、ロック中の全設定から書き込む `(key, value)` を決める関数。
pub type SiteSettingsEdit =
    Box<dyn FnOnce(&HashMap<String, String>) -> Result<Vec<(String, String)>, EditConflict> + Send>;

#[async_trait]
pub trait SiteSettingsRepository: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<String>, sqlx::Error>;
    async fn set(&self, key: &str, value: &str) -> Result<(), sqlx::Error>;
    async fn get_all(&self) -> Result<HashMap<String, String>, sqlx::Error>;
    /// 全設定を読んで `edit` が決めた値を書き込むまでを1トランザクションで行い、書き込み後の
    /// 全設定を返す。テーブルを `SHARE ROW EXCLUSIVE` でロックして書き込み同士を直列化する
    /// （まだ行の無いキーもあるので行ロックでは足りない。読むだけの `SELECT` は待たせない）。
    /// `edit` が衝突を返したら何も書かない。
    async fn update_with(
        &self,
        edit: SiteSettingsEdit,
    ) -> Result<Result<HashMap<String, String>, EditConflict>, sqlx::Error>;
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
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM site_settings WHERE key = $1")
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
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM site_settings")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().collect())
    }

    async fn update_with(
        &self,
        edit: SiteSettingsEdit,
    ) -> Result<Result<HashMap<String, String>, EditConflict>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("LOCK TABLE site_settings IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await?;
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM site_settings")
            .fetch_all(&mut *tx)
            .await?;
        let mut settings: HashMap<String, String> = rows.into_iter().collect();
        let pairs = match edit(&settings) {
            Ok(pairs) => pairs,
            Err(conflict) => return Ok(Err(conflict)),
        };
        if !pairs.is_empty() {
            let (keys, values): (Vec<String>, Vec<String>) = pairs.iter().cloned().unzip();
            sqlx::query(
                "INSERT INTO site_settings (key, value, updated_at)
                 SELECT k, v, NOW() FROM unnest($1::text[], $2::text[]) AS t(k, v)
                 ON CONFLICT (key) DO UPDATE
                   SET value = EXCLUDED.value,
                       updated_at = NOW()",
            )
            .bind(&keys)
            .bind(&values)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        settings.extend(pairs);
        Ok(Ok(settings))
    }
}

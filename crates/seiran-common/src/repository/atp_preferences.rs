use async_trait::async_trait;
use sqlx::PgPool;

#[async_trait]
pub trait AtpPreferencesRepository: Send + Sync {
    /// `app.bsky.actor.getPreferences` — 未保存なら空配列を返す。
    async fn get(&self, actor_id: i64) -> Result<serde_json::Value, sqlx::Error>;

    /// `app.bsky.actor.putPreferences` — 全置換（`preferences`配列を丸ごと差し替える）。
    async fn put(&self, actor_id: i64, preferences: &serde_json::Value) -> Result<(), sqlx::Error>;
}

pub struct PgAtpPreferencesRepository {
    pool: PgPool,
}

impl PgAtpPreferencesRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AtpPreferencesRepository for PgAtpPreferencesRepository {
    async fn get(&self, actor_id: i64) -> Result<serde_json::Value, sqlx::Error> {
        let row: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT preferences FROM atp_preferences WHERE actor_id = $1")
                .bind(actor_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.unwrap_or_else(|| serde_json::json!([])))
    }

    async fn put(&self, actor_id: i64, preferences: &serde_json::Value) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO atp_preferences (actor_id, preferences, updated_at)
             VALUES ($1, $2, NOW())
             ON CONFLICT (actor_id) DO UPDATE SET preferences = EXCLUDED.preferences, updated_at = NOW()",
        )
        .bind(actor_id)
        .bind(preferences)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }
}

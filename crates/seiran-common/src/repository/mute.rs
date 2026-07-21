use async_trait::async_trait;
use sqlx::PgPool;

#[async_trait]
pub trait MuteRepository: Send + Sync {
    /// ミュートを挿入する（既存なら何もしない）。AP/ATP 配送は発生しないローカル効果のみ。
    async fn insert(&self, muter_actor_id: i64, muted_actor_id: i64) -> Result<(), sqlx::Error>;

    async fn delete_by_actors(
        &self,
        muter_actor_id: i64,
        muted_actor_id: i64,
    ) -> Result<(), sqlx::Error>;

    async fn is_muted(&self, muter_actor_id: i64, muted_actor_id: i64) -> Result<bool, sqlx::Error>;
}

pub struct PgMuteRepository {
    pool: PgPool,
}

impl PgMuteRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MuteRepository for PgMuteRepository {
    async fn insert(&self, muter_actor_id: i64, muted_actor_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO mutes (muter_actor_id, muted_actor_id)
             VALUES ($1, $2)
             ON CONFLICT (muter_actor_id, muted_actor_id) DO NOTHING",
        )
        .bind(muter_actor_id)
        .bind(muted_actor_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn delete_by_actors(
        &self,
        muter_actor_id: i64,
        muted_actor_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "DELETE FROM mutes WHERE muter_actor_id = $1 AND muted_actor_id = $2",
        )
        .bind(muter_actor_id)
        .bind(muted_actor_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn is_muted(&self, muter_actor_id: i64, muted_actor_id: i64) -> Result<bool, sqlx::Error> {
        let row: (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM mutes WHERE muter_actor_id = $1 AND muted_actor_id = $2)",
        )
        .bind(muter_actor_id)
        .bind(muted_actor_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }
}

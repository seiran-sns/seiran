use async_trait::async_trait;
use sqlx::PgPool;

#[async_trait]
pub trait FollowRepository: Send + Sync {
    /// フォローを pending で挿入する（既存なら status を pending に戻す）。
    async fn upsert_pending(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<(), sqlx::Error>;

    /// フォロー関係の status を取得する（未フォローなら None）。
    async fn find_status(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<Option<String>, sqlx::Error>;
}

pub struct PgFollowRepository {
    pool: PgPool,
}

impl PgFollowRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl FollowRepository for PgFollowRepository {
    async fn upsert_pending(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO follows (follower_actor_id, target_actor_id, status)
             VALUES ($1, $2, 'pending')
             ON CONFLICT (follower_actor_id, target_actor_id) DO UPDATE
               SET status = 'pending'",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn find_status(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT status FROM follows
             WHERE follower_actor_id = $1 AND target_actor_id = $2 LIMIT 1",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }
}

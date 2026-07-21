use async_trait::async_trait;
use sqlx::PgPool;

#[async_trait]
pub trait BlockRepository: Send + Sync {
    /// ブロックを挿入する（rkey があれば保存）。既存なら atp_rkey を上書きする。
    async fn insert(
        &self,
        blocker_actor_id: i64,
        blocked_actor_id: i64,
        atp_rkey: Option<&str>,
    ) -> Result<(), sqlx::Error>;

    /// ブロック関係を削除する。
    async fn delete_by_actors(
        &self,
        blocker_actor_id: i64,
        blocked_actor_id: i64,
    ) -> Result<(), sqlx::Error>;

    /// ブロック時に保存した atp_rkey を取得する（アンブロック時の ATP 削除に使用）。
    async fn find_atp_rkey(
        &self,
        blocker_actor_id: i64,
        blocked_actor_id: i64,
    ) -> Result<Option<String>, sqlx::Error>;

    /// (is_blocking, is_blocked_by) を1クエリで返す。
    /// is_blocking: actor_a が actor_b をブロックしているか。
    /// is_blocked_by: actor_b が actor_a をブロックしているか。
    async fn find_relationship(
        &self,
        actor_a: i64,
        actor_b: i64,
    ) -> Result<(bool, bool), sqlx::Error>;
}

pub struct PgBlockRepository {
    pool: PgPool,
}

impl PgBlockRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl BlockRepository for PgBlockRepository {
    async fn insert(
        &self,
        blocker_actor_id: i64,
        blocked_actor_id: i64,
        atp_rkey: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO blocks (blocker_actor_id, blocked_actor_id, atp_rkey)
             VALUES ($1, $2, $3)
             ON CONFLICT (blocker_actor_id, blocked_actor_id) DO UPDATE
               SET atp_rkey = EXCLUDED.atp_rkey",
        )
        .bind(blocker_actor_id)
        .bind(blocked_actor_id)
        .bind(atp_rkey)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn delete_by_actors(
        &self,
        blocker_actor_id: i64,
        blocked_actor_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "DELETE FROM blocks WHERE blocker_actor_id = $1 AND blocked_actor_id = $2",
        )
        .bind(blocker_actor_id)
        .bind(blocked_actor_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn find_atp_rkey(
        &self,
        blocker_actor_id: i64,
        blocked_actor_id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT atp_rkey FROM blocks
             WHERE blocker_actor_id = $1 AND blocked_actor_id = $2 LIMIT 1",
        )
        .bind(blocker_actor_id)
        .bind(blocked_actor_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|r| r.0))
    }

    async fn find_relationship(
        &self,
        actor_a: i64,
        actor_b: i64,
    ) -> Result<(bool, bool), sqlx::Error> {
        let row: (bool, bool) = sqlx::query_as(
            "SELECT
               EXISTS(SELECT 1 FROM blocks WHERE blocker_actor_id = $1 AND blocked_actor_id = $2) AS is_blocking,
               EXISTS(SELECT 1 FROM blocks WHERE blocker_actor_id = $2 AND blocked_actor_id = $1) AS is_blocked_by",
        )
        .bind(actor_a)
        .bind(actor_b)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }
}

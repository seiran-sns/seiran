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

    /// リモートからのフォロー受信時に accepted で挿入する（重複なら何もしない）。
    async fn insert_accepted(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<(), sqlx::Error>;

    /// pending のフォローを accepted に昇格させる（Accept 受信時）。
    async fn accept(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<u64, sqlx::Error>;

    /// フォロー関係を削除する（Undo/Follow 受信時）。
    async fn delete_by_actors(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<(), sqlx::Error>;

    /// フォロー関係の atp_rkey を取得する（アンフォロー時の ATP 削除に使用）。
    async fn find_atp_rkey(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<Option<String>, sqlx::Error>;

    /// ATP フォロー完了後に accepted で挿入する（rkey を保存）。
    /// 既にフォロー済みの場合は何もしない。
    async fn insert_accepted_bsky(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
        atp_rkey: &str,
    ) -> Result<(), sqlx::Error>;

    /// `target_actor_id` を accepted な status でフォローしているローカルアクターの ID 一覧を取得する。
    /// 新規投稿の realtime WebSocket 配信対象を決めるために使う。
    async fn find_accepted_local_follower_ids(
        &self,
        target_actor_id: i64,
    ) -> Result<Vec<i64>, sqlx::Error>;

    /// `follower_actor_id` が accepted な status でフォローしている全ての
    /// `target_actor_id` を取得する（退会時、フォロー先全員への一括アンフォロー用）。
    async fn find_accepted_target_ids(
        &self,
        follower_actor_id: i64,
    ) -> Result<Vec<i64>, sqlx::Error>;
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

    async fn insert_accepted(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO follows (follower_actor_id, target_actor_id, status)
             VALUES ($1, $2, 'accepted')
             ON CONFLICT (follower_actor_id, target_actor_id) DO NOTHING",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn accept(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE follows SET status = 'accepted'
             WHERE follower_actor_id = $1 AND target_actor_id = $2 AND status = 'pending'",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn delete_by_actors(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "DELETE FROM follows WHERE follower_actor_id = $1 AND target_actor_id = $2",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn find_atp_rkey(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT atp_rkey FROM follows
             WHERE follower_actor_id = $1 AND target_actor_id = $2 LIMIT 1",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|r| r.0))
    }

    async fn insert_accepted_bsky(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
        atp_rkey: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO follows (follower_actor_id, target_actor_id, status, atp_rkey)
             VALUES ($1, $2, 'accepted', $3)
             ON CONFLICT (follower_actor_id, target_actor_id) DO NOTHING",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .bind(atp_rkey)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn find_accepted_local_follower_ids(
        &self,
        target_actor_id: i64,
    ) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            "SELECT f.follower_actor_id FROM follows f
             JOIN actors a ON a.id = f.follower_actor_id
             WHERE f.target_actor_id = $1 AND f.status = 'accepted' AND a.actor_type = 'local'",
        )
        .bind(target_actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_accepted_target_ids(
        &self,
        follower_actor_id: i64,
    ) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            "SELECT target_actor_id FROM follows
             WHERE follower_actor_id = $1 AND status = 'accepted'",
        )
        .bind(follower_actor_id)
        .fetch_all(&self.pool)
        .await
    }
}

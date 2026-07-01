use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// `atp_repo_events` テーブルの 1 行（subscribeRepos のリプレイ用）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RepoEvent {
    pub id: i64,
    pub did: String,
    pub commit_cid: String,
    pub prev_cid: Option<String>,
    pub rev: String,
    pub since_rev: Option<String>,
    pub car_bytes: Vec<u8>,
    pub ops_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait AtpReadRepository: Send + Sync {
    /// 指定アクターの全 CAR ブロック (cid, bytes) を取得する。
    async fn find_blocks_by_actor(
        &self,
        actor_id: i64,
    ) -> Result<Vec<(String, Vec<u8>)>, sqlx::Error>;

    /// cursor より後のリポジトリイベントを取得する（subscribeRepos のバックフィル）。
    async fn find_events_after(
        &self,
        cursor: i64,
        limit: i64,
    ) -> Result<Vec<RepoEvent>, sqlx::Error>;
}

pub struct PgAtpReadRepository {
    pool: PgPool,
}

impl PgAtpReadRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AtpReadRepository for PgAtpReadRepository {
    async fn find_blocks_by_actor(
        &self,
        actor_id: i64,
    ) -> Result<Vec<(String, Vec<u8>)>, sqlx::Error> {
        sqlx::query_as::<_, (String, Vec<u8>)>(
            "SELECT cid, bytes FROM atp_blocks WHERE actor_id = $1",
        )
        .bind(actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_events_after(
        &self,
        cursor: i64,
        limit: i64,
    ) -> Result<Vec<RepoEvent>, sqlx::Error> {
        sqlx::query_as::<_, RepoEvent>(
            "SELECT id, did, commit_cid, prev_cid, rev, since_rev, car_bytes, ops_json, created_at
             FROM atp_repo_events WHERE id > $1 ORDER BY id ASC LIMIT $2",
        )
        .bind(cursor)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
}

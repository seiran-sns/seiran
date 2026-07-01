use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// タイムライン表示用のポスト + アクター結合行。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TimelinePost {
    pub id: i64,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub actor_id: i64,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
}

/// プロフィール表示用のポスト要約。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PostSummary {
    pub id: i64,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

/// XRPC getRecord 用のレコード行。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PostRecord {
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub at_uri: String,
    pub at_cid: String,
}

#[async_trait]
pub trait PostRepository: Send + Sync {
    /// 新規ポストを挿入する。
    async fn insert(
        &self,
        id: i64,
        actor_id: i64,
        body: &str,
        ap_object_id: &str,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// ホームタイムライン（自分 + フォロー中の accepted アクターの投稿）を取得する。
    async fn home_timeline(
        &self,
        actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// ローカルタイムライン（ローカルアクターの投稿）を取得する。
    async fn local_timeline(
        &self,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// 指定アクターの最近の投稿を取得する。
    async fn recent_by_actor(
        &self,
        actor_id: i64,
        limit: i64,
    ) -> Result<Vec<PostSummary>, sqlx::Error>;

    /// DID + rkey で app.bsky.feed.post レコードを取得する。
    async fn find_record(&self, did: &str, rkey: &str) -> Result<Option<PostRecord>, sqlx::Error>;

    /// ID でポストとアクター情報を取得する。
    async fn find_by_id(&self, id: i64) -> Result<Option<TimelinePost>, sqlx::Error>;
}

pub struct PgPostRepository {
    pool: PgPool,
}

impl PgPostRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PostRepository for PgPostRepository {
    async fn insert(
        &self,
        id: i64,
        actor_id: i64,
        body: &str,
        ap_object_id: &str,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO posts (id, actor_id, body, ap_object_id, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(actor_id)
        .bind(body)
        .bind(ap_object_id)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn home_timeline(
        &self,
        actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name
             FROM posts p JOIN actors a ON a.id = p.actor_id
             WHERE p.deleted_at IS NULL
               AND ($2::bigint IS NULL OR p.id < $2)
               AND ($3::bigint IS NULL OR p.id > $3)
               AND (p.actor_id = $1 OR p.actor_id IN (
                     SELECT target_actor_id FROM follows
                     WHERE follower_actor_id = $1 AND status = 'accepted'))
             ORDER BY p.id DESC LIMIT $4",
        )
        .bind(actor_id)
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn local_timeline(
        &self,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name
             FROM posts p JOIN actors a ON a.id = p.actor_id
             WHERE a.actor_type = 'local' AND p.deleted_at IS NULL
               AND ($1::bigint IS NULL OR p.id < $1)
               AND ($2::bigint IS NULL OR p.id > $2)
             ORDER BY p.id DESC LIMIT $3",
        )
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn recent_by_actor(
        &self,
        actor_id: i64,
        limit: i64,
    ) -> Result<Vec<PostSummary>, sqlx::Error> {
        sqlx::query_as::<_, PostSummary>(
            "SELECT id, body, created_at FROM posts
             WHERE actor_id = $1 AND deleted_at IS NULL
             ORDER BY id DESC LIMIT $2",
        )
        .bind(actor_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_record(&self, did: &str, rkey: &str) -> Result<Option<PostRecord>, sqlx::Error> {
        sqlx::query_as::<_, PostRecord>(
            "SELECT p.body, p.created_at, p.at_uri, p.at_cid
             FROM posts p
             JOIN actors a ON a.id = p.actor_id
             WHERE a.at_did = $1 AND p.at_rkey = $2 AND p.deleted_at IS NULL
             LIMIT 1",
        )
        .bind(did)
        .bind(rkey)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_by_id(&self, id: i64) -> Result<Option<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name
             FROM posts p JOIN actors a ON a.id = p.actor_id
             WHERE p.id = $1 AND p.deleted_at IS NULL
             LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }
}

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// `actors` テーブルの 1 行（アプリで使用するカラムのみ）。
///
/// PostgreSQL の `actor_type_enum` は SELECT 時に `::text` キャストして `String` に
/// デコードする（`ACTOR_COLS` 参照）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Actor {
    pub id: i64,
    pub user_id: Option<i64>,
    pub actor_type: String,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub ap_uri: Option<String>,
    pub ap_inbox_url: Option<String>,
    pub at_did: Option<String>,
    pub at_repo_cid: Option<String>,
    pub at_repo_rev: Option<String>,
    pub at_signing_key_pem: Option<String>,
}

/// `Actor` の全フィールドに対応する SELECT カラム列。`actor_type` は enum のため text にキャストする。
const ACTOR_COLS: &str = "id, user_id, actor_type::text AS actor_type, username, domain, \
    display_name, ap_uri, ap_inbox_url, at_did, at_repo_cid, at_repo_rev, at_signing_key_pem";

#[async_trait]
pub trait ActorRepository: Send + Sync {
    /// ローカルユーザー（`actor_type = 'local'`）のアクターを user_id で取得する。
    async fn find_local_by_user_id(&self, user_id: i64) -> Result<Option<Actor>, sqlx::Error>;

    /// ユーザー名 + ドメインでアクターを取得する。
    async fn find_by_username_domain(
        &self,
        username: &str,
        domain: &str,
    ) -> Result<Option<Actor>, sqlx::Error>;

    /// ActivityPub Actor URI でアクターを取得する。
    async fn find_by_ap_uri(&self, ap_uri: &str) -> Result<Option<Actor>, sqlx::Error>;

    /// AT Protocol DID でアクターを取得する。
    async fn find_by_did(&self, did: &str) -> Result<Option<Actor>, sqlx::Error>;

    /// ユーザー名 + ドメインから DID のみを取得する（`at_did IS NOT NULL` のもの）。
    async fn find_did_by_username_domain(
        &self,
        username: &str,
        domain: &str,
    ) -> Result<Option<String>, sqlx::Error>;

    /// 新規ローカルアクターを挿入する。
    async fn insert_local(
        &self,
        id: i64,
        user_id: i64,
        username: &str,
        domain: &str,
        at_did: &str,
        at_signing_key_pem: &str,
    ) -> Result<(), sqlx::Error>;

    /// リモート（Fediverse）アクターを upsert し、その actor_id を返す。
    #[allow(clippy::too_many_arguments)]
    async fn upsert_remote_fedi(
        &self,
        id: i64,
        ap_uri: &str,
        ap_inbox_url: &str,
        username: &str,
        domain: &str,
        display_name: &str,
        now: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error>;

    /// DID を持つ全ローカルアクターの (username, did) を取得する（起動時 TXT 再登録用）。
    async fn list_local_dids(&self) -> Result<Vec<(String, String)>, sqlx::Error>;
}

pub struct PgActorRepository {
    pool: PgPool,
}

impl PgActorRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ActorRepository for PgActorRepository {
    async fn find_local_by_user_id(&self, user_id: i64) -> Result<Option<Actor>, sqlx::Error> {
        sqlx::query_as::<_, Actor>(&format!(
            "SELECT {ACTOR_COLS} FROM actors WHERE user_id = $1 AND actor_type = 'local' LIMIT 1"
        ))
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_by_username_domain(
        &self,
        username: &str,
        domain: &str,
    ) -> Result<Option<Actor>, sqlx::Error> {
        sqlx::query_as::<_, Actor>(&format!(
            "SELECT {ACTOR_COLS} FROM actors WHERE username = $1 AND domain = $2 LIMIT 1"
        ))
        .bind(username)
        .bind(domain)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_by_ap_uri(&self, ap_uri: &str) -> Result<Option<Actor>, sqlx::Error> {
        sqlx::query_as::<_, Actor>(&format!(
            "SELECT {ACTOR_COLS} FROM actors WHERE ap_uri = $1 LIMIT 1"
        ))
        .bind(ap_uri)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_by_did(&self, did: &str) -> Result<Option<Actor>, sqlx::Error> {
        sqlx::query_as::<_, Actor>(&format!(
            "SELECT {ACTOR_COLS} FROM actors WHERE at_did = $1 LIMIT 1"
        ))
        .bind(did)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_did_by_username_domain(
        &self,
        username: &str,
        domain: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT at_did FROM actors
             WHERE username = $1 AND domain = $2 AND at_did IS NOT NULL LIMIT 1",
        )
        .bind(username)
        .bind(domain)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }

    async fn insert_local(
        &self,
        id: i64,
        user_id: i64,
        username: &str,
        domain: &str,
        at_did: &str,
        at_signing_key_pem: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO actors (id, user_id, actor_type, username, domain, at_did, at_signing_key_pem, created_at, updated_at)
             VALUES ($1, $2, 'local', $3, $4, $5, $6, NOW(), NOW())",
        )
        .bind(id)
        .bind(user_id)
        .bind(username)
        .bind(domain)
        .bind(at_did)
        .bind(at_signing_key_pem)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn upsert_remote_fedi(
        &self,
        id: i64,
        ap_uri: &str,
        ap_inbox_url: &str,
        username: &str,
        domain: &str,
        display_name: &str,
        now: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO actors (id, actor_type, ap_uri, ap_inbox_url, username, domain, display_name, created_at, updated_at)
             VALUES ($1, 'fedi', $2, $3, $4, $5, $6, $7, $7)
             ON CONFLICT (ap_uri) DO UPDATE
               SET ap_inbox_url = EXCLUDED.ap_inbox_url,
                   display_name = EXCLUDED.display_name,
                   updated_at   = EXCLUDED.updated_at
             RETURNING id",
        )
        .bind(id)
        .bind(ap_uri)
        .bind(ap_inbox_url)
        .bind(username)
        .bind(domain)
        .bind(display_name)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    async fn list_local_dids(&self) -> Result<Vec<(String, String)>, sqlx::Error> {
        sqlx::query_as::<_, (String, String)>(
            "SELECT username, at_did FROM actors
             WHERE actor_type = 'local' AND at_did IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await
    }
}

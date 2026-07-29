//! Fediverseリレー（#140）の登録・状態管理。
//!
//! リレー本体は `actors` テーブルには登録しない（Mastodon本家のリレー実装と同様、
//! 管理者が入力した1つの inbox URL を Follow の object と実配送先の両方に使う）。
//! 相手側からの Accept/Reject/Undo は `follow_activity_id` の一致で本テーブルを直接更新する。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayStatus {
    /// Follow送信済み・Accept待ち
    Pending,
    /// Acceptされ、配送対象になっている
    Accepted,
    /// Rejectされた、または削除前にUndoした
    Rejected,
}

impl RelayStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RelayStatus::Pending => "pending",
            RelayStatus::Accepted => "accepted",
            RelayStatus::Rejected => "rejected",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(RelayStatus::Pending),
            "accepted" => Some(RelayStatus::Accepted),
            "rejected" => Some(RelayStatus::Rejected),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Relay {
    pub id: i64,
    pub inbox_url: String,
    pub status: RelayStatus,
    pub follow_activity_id: String,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("DB エラー: {0}")]
    Db(#[from] sqlx::Error),
    #[error("同じinbox URLのリレーが既に登録されています")]
    DuplicateInboxUrl,
}

#[async_trait]
pub trait RelayRepository: Send + Sync {
    async fn list_all(&self) -> Result<Vec<Relay>, RelayError>;
    /// 配送対象（status='accepted'）のinbox URL一覧。
    async fn list_accepted_inbox_urls(&self) -> Result<Vec<String>, RelayError>;
    async fn find_by_id(&self, id: i64) -> Result<Option<Relay>, RelayError>;
    async fn find_by_follow_activity_id(
        &self,
        follow_activity_id: &str,
    ) -> Result<Option<Relay>, RelayError>;
    /// `local_domain` から `follow_activity_id` を組み立てて挿入する
    /// （id は内部で採番するため、呼び出し側は事前に follow_activity_id を組み立てられない）。
    async fn insert(&self, inbox_url: &str, local_domain: &str) -> Result<Relay, RelayError>;
    async fn update_status(
        &self,
        id: i64,
        status: RelayStatus,
        last_error: Option<&str>,
    ) -> Result<(), RelayError>;
    async fn delete(&self, id: i64) -> Result<(), RelayError>;
}

pub struct PgRelayRepository {
    pool: PgPool,
}

impl PgRelayRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(sqlx::FromRow)]
struct RelayRow {
    id: i64,
    inbox_url: String,
    status: String,
    follow_activity_id: String,
    last_error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<RelayRow> for Relay {
    type Error = RelayError;

    fn try_from(row: RelayRow) -> Result<Self, Self::Error> {
        let status = RelayStatus::parse(&row.status).ok_or_else(|| {
            RelayError::Db(sqlx::Error::Decode(
                format!("不明な relay status: {}", row.status).into(),
            ))
        })?;
        Ok(Relay {
            id: row.id,
            inbox_url: row.inbox_url,
            status,
            follow_activity_id: row.follow_activity_id,
            last_error: row.last_error,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[async_trait]
impl RelayRepository for PgRelayRepository {
    async fn list_all(&self) -> Result<Vec<Relay>, RelayError> {
        let rows = sqlx::query_as::<_, RelayRow>(
            "SELECT id, inbox_url, status, follow_activity_id, last_error, created_at, updated_at
             FROM fediverse_relays
             ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Relay::try_from).collect()
    }

    async fn list_accepted_inbox_urls(&self) -> Result<Vec<String>, RelayError> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT inbox_url FROM fediverse_relays WHERE status = 'accepted'")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(url,)| url).collect())
    }

    async fn find_by_id(&self, id: i64) -> Result<Option<Relay>, RelayError> {
        let row = sqlx::query_as::<_, RelayRow>(
            "SELECT id, inbox_url, status, follow_activity_id, last_error, created_at, updated_at
             FROM fediverse_relays WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(Relay::try_from).transpose()
    }

    async fn find_by_follow_activity_id(
        &self,
        follow_activity_id: &str,
    ) -> Result<Option<Relay>, RelayError> {
        let row = sqlx::query_as::<_, RelayRow>(
            "SELECT id, inbox_url, status, follow_activity_id, last_error, created_at, updated_at
             FROM fediverse_relays WHERE follow_activity_id = $1",
        )
        .bind(follow_activity_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(Relay::try_from).transpose()
    }

    async fn insert(&self, inbox_url: &str, local_domain: &str) -> Result<Relay, RelayError> {
        let id = crate::generate_snowflake_id(Utc::now());
        let follow_activity_id = format!("https://{}/activities/follow/relay-{}", local_domain, id);
        let row = sqlx::query_as::<_, RelayRow>(
            "INSERT INTO fediverse_relays (id, inbox_url, status, follow_activity_id)
             VALUES ($1, $2, 'pending', $3)
             RETURNING id, inbox_url, status, follow_activity_id, last_error, created_at, updated_at",
        )
        .bind(id)
        .bind(inbox_url)
        .bind(&follow_activity_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                RelayError::DuplicateInboxUrl
            }
            _ => RelayError::Db(e),
        })?;
        Relay::try_from(row)
    }

    async fn update_status(
        &self,
        id: i64,
        status: RelayStatus,
        last_error: Option<&str>,
    ) -> Result<(), RelayError> {
        sqlx::query(
            "UPDATE fediverse_relays SET status = $2, last_error = $3, updated_at = NOW()
             WHERE id = $1",
        )
        .bind(id)
        .bind(status.as_str())
        .bind(last_error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, id: i64) -> Result<(), RelayError> {
        sqlx::query("DELETE FROM fediverse_relays WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

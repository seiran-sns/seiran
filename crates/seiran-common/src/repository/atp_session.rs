use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

/// 発行済みアプリパスワード（`com.atproto.server.listAppPasswords`）の一覧表示用情報。
/// パスワード本体（ハッシュ含む）は含まない。
#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct AppPasswordRow {
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait AtpSessionRepository: Send + Sync {
    /// アプリパスワードを新規発行する（`com.atproto.server.createAppPassword`）。
    async fn insert_app_password(
        &self,
        id: i64,
        actor_id: i64,
        name: &str,
        password_hash: &str,
    ) -> Result<(), sqlx::Error>;

    /// 発行済みアプリパスワードを新しい順に返す（`com.atproto.server.listAppPasswords`）。
    async fn list_app_passwords(&self, actor_id: i64) -> Result<Vec<AppPasswordRow>, sqlx::Error>;

    /// 名前指定で無効化する（`com.atproto.server.revokeAppPassword`）。無効化できたら true。
    async fn revoke_app_password(&self, actor_id: i64, name: &str) -> Result<bool, sqlx::Error>;

    /// `createSession` でのパスワード照合用。対象アクターの有効なアプリパスワードハッシュを
    /// 全件返す（名前を問わず、identifier一致分すべてに対して照合を試みる）。
    async fn find_active_password_hashes(&self, actor_id: i64) -> Result<Vec<String>, sqlx::Error>;

    /// `refreshSession` 発行時にリフレッシュトークンの jti を記録する。
    async fn insert_refresh_token(
        &self,
        jti: Uuid,
        actor_id: i64,
        expires_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// jti が有効（未失効・未期限切れ）なら actor_id を返す。
    async fn find_valid_refresh_token_actor(&self, jti: Uuid) -> Result<Option<i64>, sqlx::Error>;

    /// jti を失効させる（`deleteSession`、または `refreshSession` 時の旧トークンローテーション）。
    async fn revoke_refresh_token(&self, jti: Uuid) -> Result<(), sqlx::Error>;
}

pub struct PgAtpSessionRepository {
    pool: PgPool,
}

impl PgAtpSessionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AtpSessionRepository for PgAtpSessionRepository {
    async fn insert_app_password(
        &self,
        id: i64,
        actor_id: i64,
        name: &str,
        password_hash: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO atp_app_passwords (id, actor_id, name, password_hash) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(actor_id)
        .bind(name)
        .bind(password_hash)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn list_app_passwords(&self, actor_id: i64) -> Result<Vec<AppPasswordRow>, sqlx::Error> {
        sqlx::query_as::<_, AppPasswordRow>(
            "SELECT name, created_at FROM atp_app_passwords
             WHERE actor_id = $1 AND revoked_at IS NULL
             ORDER BY created_at DESC",
        )
        .bind(actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn revoke_app_password(&self, actor_id: i64, name: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE atp_app_passwords SET revoked_at = now()
             WHERE actor_id = $1 AND name = $2 AND revoked_at IS NULL",
        )
        .bind(actor_id)
        .bind(name)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn find_active_password_hashes(&self, actor_id: i64) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar::<_, String>(
            "SELECT password_hash FROM atp_app_passwords WHERE actor_id = $1 AND revoked_at IS NULL",
        )
        .bind(actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn insert_refresh_token(
        &self,
        jti: Uuid,
        actor_id: i64,
        expires_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO atp_refresh_tokens (jti, actor_id, expires_at) VALUES ($1, $2, $3)",
        )
        .bind(jti)
        .bind(actor_id)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn find_valid_refresh_token_actor(&self, jti: Uuid) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            "SELECT actor_id FROM atp_refresh_tokens
             WHERE jti = $1 AND revoked_at IS NULL AND expires_at > now()",
        )
        .bind(jti)
        .fetch_optional(&self.pool)
        .await
    }

    async fn revoke_refresh_token(&self, jti: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE atp_refresh_tokens SET revoked_at = now() WHERE jti = $1")
            .bind(jti)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }
}

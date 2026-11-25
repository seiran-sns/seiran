use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

/// 発行済みアプリパスワード（`com.atproto.server.listAppPasswords`）の一覧表示用情報。
/// パスワード本体（ハッシュ含む）は含まない。
#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct AppPasswordRow {
    pub id: i64,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

/// `createSession` の照合対象になる有効なアプリパスワード。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ActiveAppPassword {
    pub id: i64,
    pub password_hash: String,
}

/// 使い切った refreshJwt の持ち主。`app_password_id` はそのセッションを始めた
/// アプリパスワード（メインパスワードなら None）で、更新後のセッションにも引き継ぐ。
#[derive(Debug, Clone, Copy, sqlx::FromRow)]
pub struct RefreshTokenOwner {
    pub actor_id: i64,
    pub app_password_id: Option<i64>,
}

#[async_trait]
pub trait AtpSessionRepository: Send + Sync {
    /// アプリパスワードを新規発行する（`com.atproto.server.createAppPassword`）。
    /// 有効なものに同じ名前があれば発行せず false を返す。
    async fn insert_app_password(
        &self,
        id: i64,
        actor_id: i64,
        name: &str,
        password_hash: &str,
    ) -> Result<bool, sqlx::Error>;

    /// 発行済みアプリパスワードを新しい順に返す（`com.atproto.server.listAppPasswords`）。
    async fn list_app_passwords(&self, actor_id: i64) -> Result<Vec<AppPasswordRow>, sqlx::Error>;

    /// 名前指定で無効化する（`com.atproto.server.revokeAppPassword`）。無効化できたら true。
    /// そのパスワードで始めたセッションの refreshJwt も同じ文で失効させる。
    async fn revoke_app_password(&self, actor_id: i64, name: &str) -> Result<bool, sqlx::Error>;

    /// id 指定で無効化する（設定画面）。挙動は `revoke_app_password` と同じ。
    async fn revoke_app_password_by_id(&self, actor_id: i64, id: i64) -> Result<bool, sqlx::Error>;

    /// `createSession` でのパスワード照合用。対象アクターの有効なアプリパスワードを
    /// 全件返す（名前を問わず、identifier一致分すべてに対して照合を試みる）。
    async fn find_active_app_passwords(
        &self,
        actor_id: i64,
    ) -> Result<Vec<ActiveAppPassword>, sqlx::Error>;

    /// `createSession`/`refreshSession` 発行時にリフレッシュトークンの jti を記録する。
    async fn insert_refresh_token(
        &self,
        jti: Uuid,
        actor_id: i64,
        app_password_id: Option<i64>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// `refreshSession` のローテーション。jti が有効（未失効・未期限切れで、アプリパスワード
    /// 由来ならそのパスワードも有効）なら失効させて持ち主を返す。確認と失効を1文で行うので、
    /// 同じ refreshJwt で同時に更新されても新しいセッションは1つしかできない。
    async fn consume_refresh_token(
        &self,
        jti: Uuid,
    ) -> Result<Option<RefreshTokenOwner>, sqlx::Error>;

    /// jti を失効させる（`deleteSession`）。
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
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "INSERT INTO atp_app_passwords (id, actor_id, name, password_hash) VALUES ($1, $2, $3, $4)
             ON CONFLICT (actor_id, name) WHERE revoked_at IS NULL DO NOTHING",
        )
        .bind(id)
        .bind(actor_id)
        .bind(name)
        .bind(password_hash)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_app_passwords(&self, actor_id: i64) -> Result<Vec<AppPasswordRow>, sqlx::Error> {
        sqlx::query_as::<_, AppPasswordRow>(
            "SELECT id, name, created_at FROM atp_app_passwords
             WHERE actor_id = $1 AND revoked_at IS NULL
             ORDER BY created_at DESC",
        )
        .bind(actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn revoke_app_password(&self, actor_id: i64, name: &str) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            "WITH revoked AS (
                 UPDATE atp_app_passwords SET revoked_at = now()
                 WHERE actor_id = $1 AND name = $2 AND revoked_at IS NULL
                 RETURNING id
             ), sessions AS (
                 UPDATE atp_refresh_tokens rt SET revoked_at = now()
                 FROM revoked
                 WHERE rt.app_password_id = revoked.id AND rt.revoked_at IS NULL
             )
             SELECT count(*) FROM revoked",
        )
        .bind(actor_id)
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map(|n| n > 0)
    }

    async fn revoke_app_password_by_id(&self, actor_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            "WITH revoked AS (
                 UPDATE atp_app_passwords SET revoked_at = now()
                 WHERE actor_id = $1 AND id = $2 AND revoked_at IS NULL
                 RETURNING id
             ), sessions AS (
                 UPDATE atp_refresh_tokens rt SET revoked_at = now()
                 FROM revoked
                 WHERE rt.app_password_id = revoked.id AND rt.revoked_at IS NULL
             )
             SELECT count(*) FROM revoked",
        )
        .bind(actor_id)
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map(|n| n > 0)
    }

    async fn find_active_app_passwords(
        &self,
        actor_id: i64,
    ) -> Result<Vec<ActiveAppPassword>, sqlx::Error> {
        sqlx::query_as::<_, ActiveAppPassword>(
            "SELECT id, password_hash FROM atp_app_passwords
             WHERE actor_id = $1 AND revoked_at IS NULL",
        )
        .bind(actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn insert_refresh_token(
        &self,
        jti: Uuid,
        actor_id: i64,
        app_password_id: Option<i64>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO atp_refresh_tokens (jti, actor_id, app_password_id, expires_at)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(jti)
        .bind(actor_id)
        .bind(app_password_id)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn consume_refresh_token(
        &self,
        jti: Uuid,
    ) -> Result<Option<RefreshTokenOwner>, sqlx::Error> {
        sqlx::query_as::<_, RefreshTokenOwner>(
            "UPDATE atp_refresh_tokens rt SET revoked_at = now()
             WHERE rt.jti = $1 AND rt.revoked_at IS NULL AND rt.expires_at > now()
               AND (rt.app_password_id IS NULL OR EXISTS (
                   SELECT 1 FROM atp_app_passwords ap
                   WHERE ap.id = rt.app_password_id AND ap.revoked_at IS NULL
               ))
             RETURNING rt.actor_id, rt.app_password_id",
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

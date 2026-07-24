use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

/// 発行済みアプリトークン（MiAuth 経由、#60）の一覧表示用情報。
#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct AppTokenRow {
    pub id: Uuid,
    pub client_name: String,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait AppTokenRepository: Send + Sync {
    /// MiAuth 認可成立時に発行したトークンを記録する。
    async fn insert(&self, id: Uuid, user_id: i64, client_name: &str) -> Result<(), sqlx::Error>;

    /// 発行済みトークンを新しい順に返す（設定画面、#60）。
    async fn list_by_user(&self, user_id: i64) -> Result<Vec<AppTokenRow>, sqlx::Error>;

    /// 本人所有のトークンのみ無効化する。無効化できたら true。
    async fn revoke(&self, id: Uuid, user_id: i64) -> Result<bool, sqlx::Error>;

    /// 認証ミドルウェアからの照会用。このテーブルに記録が無い jti（自社ログイン等、
    /// 管理対象外のトークン）は false（＝有効）を返す。
    async fn is_revoked(&self, id: Uuid) -> Result<bool, sqlx::Error>;
}

pub struct PgAppTokenRepository {
    pool: PgPool,
}

impl PgAppTokenRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AppTokenRepository for PgAppTokenRepository {
    async fn insert(&self, id: Uuid, user_id: i64, client_name: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO app_tokens (id, user_id, client_name) VALUES ($1, $2, $3)",
        )
        .bind(id)
        .bind(user_id)
        .bind(client_name)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn list_by_user(&self, user_id: i64) -> Result<Vec<AppTokenRow>, sqlx::Error> {
        sqlx::query_as::<_, AppTokenRow>(
            "SELECT id, client_name, created_at FROM app_tokens
             WHERE user_id = $1 AND revoked_at IS NULL
             ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn revoke(&self, id: Uuid, user_id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE app_tokens SET revoked_at = now()
             WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
        )
        .bind(id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn is_revoked(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let row: Option<(bool,)> = sqlx::query_as(
            "SELECT revoked_at IS NOT NULL FROM app_tokens WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(revoked,)| revoked).unwrap_or(false))
    }
}

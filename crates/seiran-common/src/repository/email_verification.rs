use async_trait::async_trait;
use sqlx::PgPool;

/// `email_verifications` テーブル（新規登録時のメール確認フロー）へのアクセス。
#[async_trait]
pub trait EmailVerificationRepository: Send + Sync {
    /// 有効なトークン（期限内）を消費し、紐づくメールアドレスを返す。
    async fn consume(&self, token: uuid::Uuid) -> Result<Option<String>, sqlx::Error>;
}

pub struct PgEmailVerificationRepository {
    pool: PgPool,
}

impl PgEmailVerificationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl EmailVerificationRepository for PgEmailVerificationRepository {
    async fn consume(&self, token: uuid::Uuid) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(
            "DELETE FROM email_verifications
             WHERE token = $1 AND expires_at > now()
             RETURNING email",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(email,)| email))
    }
}

/// メール確認のトークンを発行する。
pub async fn issue(pool: &PgPool, id: i64, email: &str) -> Result<uuid::Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO email_verifications (id, email) VALUES ($1, $2) RETURNING token",
    )
    .bind(id)
    .bind(email)
    .fetch_one(pool)
    .await
}

/// 期限内の有効なトークンか（消費はしない）。
pub async fn is_valid(pool: &PgPool, token: uuid::Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM email_verifications WHERE token = $1 AND expires_at > now())",
    )
    .bind(token)
    .fetch_one(pool)
    .await
}

use async_trait::async_trait;
use sqlx::PgPool;

/// `email_changes` テーブル（ログイン中ユーザーのメールアドレス変更フロー）へのアクセス。
#[async_trait]
pub trait EmailChangeRepository: Send + Sync {
    /// 変更リクエストを発行する。token は DB の `DEFAULT gen_random_uuid()` で生成する。
    async fn insert(&self, id: i64, user_id: i64, new_email: &str) -> Result<Option<String>, sqlx::Error>;

    /// 有効なトークン（期限内）を消費し、紐づく (user_id, new_email) を返す。
    async fn consume(&self, token: uuid::Uuid) -> Result<Option<(i64, String)>, sqlx::Error>;
}

pub struct PgEmailChangeRepository {
    pool: PgPool,
}

impl PgEmailChangeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl EmailChangeRepository for PgEmailChangeRepository {
    async fn insert(&self, id: i64, user_id: i64, new_email: &str) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(
            "INSERT INTO email_changes (id, user_id, new_email)
             VALUES ($1, $2, $3)
             RETURNING token::text",
        )
        .bind(id)
        .bind(user_id)
        .bind(new_email)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(t,)| t))
    }

    async fn consume(&self, token: uuid::Uuid) -> Result<Option<(i64, String)>, sqlx::Error> {
        let row: Option<(i64, String)> = sqlx::query_as(
            "DELETE FROM email_changes
             WHERE token = $1 AND expires_at > now()
             RETURNING user_id, new_email",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }
}

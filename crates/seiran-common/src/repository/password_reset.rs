use async_trait::async_trait;
use sqlx::PgPool;

/// `password_resets` テーブル（パスワードリセットフロー）へのアクセス。
#[async_trait]
pub trait PasswordResetRepository: Send + Sync {
    /// リセットレコードを発行する。token は DB の `DEFAULT gen_random_uuid()` で生成する。
    async fn insert(&self, id: i64, user_id: i64) -> Result<Option<String>, sqlx::Error>;

    /// 有効なトークン（未使用かつ期限内）から user_id を取得する。
    async fn find_valid_user_id(&self, token: &str) -> Result<Option<i64>, sqlx::Error>;

    /// 有効なトークンを一度だけ消費し、同じトランザクションでパスワードを更新する。
    /// トークンが無効・使用済みなら `false` を返す。
    async fn consume_and_update_password(
        &self,
        token: &str,
        password_hash: &str,
    ) -> Result<bool, sqlx::Error>;
}

pub struct PgPasswordResetRepository {
    pool: PgPool,
}

impl PgPasswordResetRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PasswordResetRepository for PgPasswordResetRepository {
    async fn insert(&self, id: i64, user_id: i64) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(
            "INSERT INTO password_resets (id, user_id)
             VALUES ($1, $2)
             RETURNING token::text",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(t,)| t))
    }

    async fn find_valid_user_id(&self, token: &str) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT user_id FROM password_resets
             WHERE token = $1::uuid
               AND used_at IS NULL
               AND expires_at > NOW()",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id,)| id))
    }

    async fn consume_and_update_password(
        &self,
        token: &str,
        password_hash: &str,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let user_id: Option<i64> = sqlx::query_scalar(
            "UPDATE password_resets
             SET used_at = NOW()
             WHERE token = $1::uuid
               AND used_at IS NULL
               AND expires_at > NOW()
             RETURNING user_id",
        )
            .bind(token)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(user_id) = user_id else {
            tx.rollback().await?;
            return Ok(false);
        };
        sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
            .bind(password_hash)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }
}

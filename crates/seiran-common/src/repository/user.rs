use async_trait::async_trait;
use sqlx::PgPool;

/// ログイン処理用の users + actors 結合行。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LoginRow {
    pub id: i64,
    pub email: String,
    pub password_hash: Option<String>,
    pub username: String,
}

#[async_trait]
pub trait UserRepository: Send + Sync {
    /// メールアドレスが登録済みかを返す。
    async fn email_exists(&self, email: &str) -> Result<bool, sqlx::Error>;

    /// 登録済みユーザー総数を返す（セットアップ状態チェック用）。
    async fn count(&self) -> Result<i64, sqlx::Error>;

    /// 新規ユーザーを挿入し、その user_id を返す。role は 'user' / 'moderator' / 'admin'。
    async fn insert(&self, email: &str, password_hash: &str, role: &str) -> Result<i64, sqlx::Error>;

    /// ログイン用にメールアドレスでユーザー + ローカルアクターを取得する。
    async fn find_login_by_email(&self, email: &str) -> Result<Option<LoginRow>, sqlx::Error>;
}

pub struct PgUserRepository {
    pool: PgPool,
}

impl PgUserRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl UserRepository for PgUserRepository {
    async fn email_exists(&self, email: &str) -> Result<bool, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE email = $1 LIMIT 1")
            .bind(email)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    async fn count(&self) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    async fn insert(&self, email: &str, password_hash: &str, role: &str) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO users (email, password_hash, role, created_at, updated_at)
             VALUES ($1, $2, $3::user_role, NOW(), NOW())
             RETURNING id",
        )
        .bind(email)
        .bind(password_hash)
        .bind(role)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    async fn find_login_by_email(&self, email: &str) -> Result<Option<LoginRow>, sqlx::Error> {
        sqlx::query_as::<_, LoginRow>(
            "SELECT u.id, u.email, u.password_hash, a.username
             FROM users u
             JOIN actors a ON a.user_id = u.id AND a.actor_type = 'local'
             WHERE u.email = $1
             LIMIT 1",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
    }
}

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// ログイン処理用の users + actors 結合行。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LoginRow {
    pub id: i64,
    pub email: String,
    pub password_hash: Option<String>,
    pub username: String,
}

/// 管理画面ユーザー一覧（`GET /api/admin/users`）用の users + actors 結合行。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AdminUserRow {
    pub id: i64,
    pub email: String,
    pub role: String,
    pub suspended_at: Option<DateTime<Utc>>,
    pub username: Option<String>,
}

#[async_trait]
pub trait UserRepository: Send + Sync {
    /// メールアドレスが登録済みかを返す。
    async fn email_exists(&self, email: &str) -> Result<bool, sqlx::Error>;

    /// 登録済みユーザー総数を返す（セットアップ状態チェック用）。
    async fn count(&self) -> Result<i64, sqlx::Error>;

    /// 新規ユーザーを挿入し、その user_id を返す。role は 'user' / 'moderator' / 'admin'。
    async fn insert(&self, email: &str, password_hash: &str, role: &str) -> Result<i64, sqlx::Error>;

    /// ユーザー ID からロール文字列（"user" / "moderator" / "admin"）を取得する。
    async fn find_role_by_user_id(&self, user_id: i64) -> Result<Option<String>, sqlx::Error>;

    /// ログイン用にメールアドレスでユーザー + ローカルアクターを取得する。
    async fn find_login_by_email(&self, email: &str) -> Result<Option<LoginRow>, sqlx::Error>;

    /// ログイン用にユーザーネームでユーザー + ローカルアクターを取得する。
    async fn find_login_by_username(&self, username: &str) -> Result<Option<LoginRow>, sqlx::Error>;

    /// メールアドレスから user_id を取得する（パスワードリセット申請用）。
    async fn find_id_by_email(&self, email: &str) -> Result<Option<i64>, sqlx::Error>;

    /// パスワードハッシュを更新する。
    async fn update_password_hash(&self, user_id: i64, password_hash: &str) -> Result<(), sqlx::Error>;

    /// メールアドレスを更新する（設定画面からのメールアドレス変更確定用）。
    async fn update_email(&self, user_id: i64, email: &str) -> Result<(), sqlx::Error>;

    /// 管理画面のユーザー一覧を返す（先頭100件、ID昇順）。
    async fn list_for_admin(&self) -> Result<Vec<AdminUserRow>, sqlx::Error>;

    /// アカウントの凍結状態を更新する。
    async fn set_suspended(&self, user_id: i64, suspended: bool) -> Result<(), sqlx::Error>;

    /// ロール（`user` / `moderator` / `admin`）を更新する。
    async fn update_role(&self, user_id: i64, role: &str) -> Result<(), sqlx::Error>;

    /// 表示言語設定（`ja` / `en`）を取得する。`None` は「自動」（ブラウザ設定に従う）。
    async fn find_language_preference_by_user_id(&self, user_id: i64) -> Result<Option<String>, sqlx::Error>;

    /// 表示言語設定を更新する。`None` を渡すと「自動」に戻す。
    async fn update_language_preference(&self, user_id: i64, language: Option<&str>) -> Result<(), sqlx::Error>;
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

    async fn find_role_by_user_id(&self, user_id: i64) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT role::text FROM users WHERE id = $1"
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(r,)| r))
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

    async fn find_login_by_username(&self, username: &str) -> Result<Option<LoginRow>, sqlx::Error> {
        sqlx::query_as::<_, LoginRow>(
            "SELECT u.id, u.email, u.password_hash, a.username
             FROM users u
             JOIN actors a ON a.user_id = u.id AND a.actor_type::text = 'local'
             WHERE a.username = $1",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_id_by_email(&self, email: &str) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE email = $1 LIMIT 1")
            .bind(email)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(id,)| id))
    }

    async fn update_password_hash(&self, user_id: i64, password_hash: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET password_hash = $1, updated_at = NOW() WHERE id = $2")
            .bind(password_hash)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn update_email(&self, user_id: i64, email: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET email = $1, updated_at = NOW() WHERE id = $2")
            .bind(email)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn list_for_admin(&self) -> Result<Vec<AdminUserRow>, sqlx::Error> {
        sqlx::query_as::<_, AdminUserRow>(
            "SELECT u.id, u.email, u.role::text AS role, u.suspended_at, a.username
             FROM users u
             LEFT JOIN actors a ON a.user_id = u.id AND a.actor_type::text = 'local'
             ORDER BY u.id
             LIMIT 100",
        )
        .fetch_all(&self.pool)
        .await
    }

    async fn set_suspended(&self, user_id: i64, suspended: bool) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET suspended_at = CASE WHEN $1 THEN NOW() ELSE NULL END WHERE id = $2")
            .bind(suspended)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn update_role(&self, user_id: i64, role: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET role = $1::user_role WHERE id = $2")
            .bind(role)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn find_language_preference_by_user_id(&self, user_id: i64) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT language_preference FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|(v,)| v))
    }

    async fn update_language_preference(&self, user_id: i64, language: Option<&str>) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET language_preference = $1, updated_at = NOW() WHERE id = $2")
            .bind(language)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }
}

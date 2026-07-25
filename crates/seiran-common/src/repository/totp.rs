use async_trait::async_trait;
use sqlx::PgPool;

/// TOTP（二段階認証）設定・リカバリーコード・メール経由の強制解除リクエストへのアクセス（#65）。
///
/// TOTPコードそのものの生成・検証（`totp-rs`）とパスワード/リカバリーコードのハッシュ照合
/// （Argon2、`LocalAuthProvider`）はこの層の責務外とし、DB操作のみを提供する。
#[async_trait]
pub trait TotpRepository: Send + Sync {
    /// setup: 暗号化済みシークレットを保存する。同一ユーザーで再実行した場合は
    /// 既存行を上書きし enabled を false に戻す（enable確定前のやり直しに対応）。
    async fn upsert_pending(
        &self,
        id: i64,
        user_id: i64,
        secret_encrypted: &str,
    ) -> Result<(), sqlx::Error>;

    /// (secret_encrypted, enabled) を返す。setup直後の確認コード検証にも、
    /// 既に有効化済みかどうかの判定にも使う。
    async fn find_by_user_id(&self, user_id: i64) -> Result<Option<(String, bool)>, sqlx::Error>;

    /// disable: 行ごと削除する（ON DELETE CASCADEでリカバリーコードも消える）。
    async fn delete(&self, user_id: i64) -> Result<(), sqlx::Error>;

    /// リカバリーコード（ハッシュ済み）を一括発行する。
    /// 既存コードの破棄、リカバリーコード発行、enable確定を同一トランザクションで行う。
    async fn enable_with_recovery_codes(
        &self,
        user_id: i64,
        codes: &[(i64, i64, String)],
    ) -> Result<(), sqlx::Error>;

    /// 未使用のリカバリーコード（id, hash）一覧を返す。照合はハッシュがArgon2で
    /// 非決定的なため呼び出し側で1件ずつ`verify_password`する。
    async fn list_unused_recovery_codes(
        &self,
        user_id: i64,
    ) -> Result<Vec<(i64, String)>, sqlx::Error>;

    /// 指定IDのリカバリーコードを使用済みにする。
    async fn mark_recovery_code_used(&self, id: i64) -> Result<bool, sqlx::Error>;

    /// メール経由の強制解除リクエストを発行し、トークン文字列を返す。
    async fn create_disable_request(
        &self,
        id: i64,
        user_id: i64,
    ) -> Result<Option<String>, sqlx::Error>;

    /// 有効なトークン（期限内）を消費し、user_id を返す。
    async fn consume_disable_request(&self, token: uuid::Uuid) -> Result<Option<i64>, sqlx::Error>;
}

pub struct PgTotpRepository {
    pool: PgPool,
}

impl PgTotpRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl TotpRepository for PgTotpRepository {
    async fn upsert_pending(
        &self,
        id: i64,
        user_id: i64,
        secret_encrypted: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO user_totp (id, user_id, secret_encrypted, enabled, confirmed_at)
             VALUES ($1, $2, $3, false, NULL)
             ON CONFLICT (user_id) DO UPDATE
             SET secret_encrypted = EXCLUDED.secret_encrypted, enabled = false, confirmed_at = NULL",
        )
        .bind(id)
        .bind(user_id)
        .bind(secret_encrypted)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn find_by_user_id(&self, user_id: i64) -> Result<Option<(String, bool)>, sqlx::Error> {
        let row: Option<(String, bool)> =
            sqlx::query_as("SELECT secret_encrypted, enabled FROM user_totp WHERE user_id = $1")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    async fn delete(&self, user_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM user_totp WHERE user_id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn enable_with_recovery_codes(
        &self,
        user_id: i64,
        codes: &[(i64, i64, String)],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM user_totp_recovery_codes WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        for (id, user_id, hash) in codes {
            sqlx::query(
                "INSERT INTO user_totp_recovery_codes (id, user_id, code_hash) VALUES ($1, $2, $3)",
            )
            .bind(id)
            .bind(user_id)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("UPDATE user_totp SET enabled = true, confirmed_at = now() WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn list_unused_recovery_codes(
        &self,
        user_id: i64,
    ) -> Result<Vec<(i64, String)>, sqlx::Error> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, code_hash FROM user_totp_recovery_codes WHERE user_id = $1 AND used_at IS NULL",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn mark_recovery_code_used(&self, id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE user_totp_recovery_codes SET used_at = now() WHERE id = $1 AND used_at IS NULL",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn create_disable_request(
        &self,
        id: i64,
        user_id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        // 直前の未消費リクエストが残っていても複数許容する（先着トークンだけ有効という
        // 状態異存を避けるため、consume側は最初に見つかった期限内トークンを消費する設計）。
        let row: Option<(String,)> = sqlx::query_as(
            "INSERT INTO totp_disable_requests (id, user_id) VALUES ($1, $2) RETURNING token::text",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(t,)| t))
    }

    async fn consume_disable_request(&self, token: uuid::Uuid) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(
            "DELETE FROM totp_disable_requests WHERE token = $1 AND expires_at > now() RETURNING user_id",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id,)| id))
    }
}

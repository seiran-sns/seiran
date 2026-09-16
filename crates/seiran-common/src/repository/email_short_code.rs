//! `email_short_codes`テーブルへのアクセス。ATPセッション2FA
//! （`com.atproto.server.createSession`の`authFactorToken`）とPLCオペレーション署名確認
//! （`com.atproto.identity.requestPlcOperationSignature`）で共有する、6桁コード型の
//! ワンタイムトークン機構（リンククリック型の`email_verifications`とは異なり、
//! ユーザーがATPクライアントへ手入力する値として使う）。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[async_trait]
pub trait EmailShortCodeRepository: Send + Sync {
    /// 新規コードを発行する（同一actor_id+purposeの未消費コードが残っていても構わない
    /// ——複数回リクエストされた場合、最後に送ったコードだけが`consume`で当たればよい）。
    async fn issue(
        &self,
        id: i64,
        actor_id: i64,
        purpose: &str,
        code_hash: &str,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// 有効なコード（期限内・`actor_id`+`purpose`一致）を消費する。ワンタイムのため
    /// 同一`actor_id`+`purpose`の残りのコード行も全て削除する（複数回リクエストした場合の
    /// 古いコードの再利用を防ぐ）。
    async fn consume(&self, actor_id: i64, purpose: &str, code_hash: &str) -> Result<bool, sqlx::Error>;
}

pub struct PgEmailShortCodeRepository {
    pool: PgPool,
}

impl PgEmailShortCodeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl EmailShortCodeRepository for PgEmailShortCodeRepository {
    async fn issue(
        &self,
        id: i64,
        actor_id: i64,
        purpose: &str,
        code_hash: &str,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO email_short_codes (id, actor_id, purpose, code_hash, expires_at, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(actor_id)
        .bind(purpose)
        .bind(code_hash)
        .bind(expires_at)
        .bind(now)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn consume(&self, actor_id: i64, purpose: &str, code_hash: &str) -> Result<bool, sqlx::Error> {
        let matched: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM email_short_codes
             WHERE actor_id = $1 AND purpose = $2 AND code_hash = $3 AND expires_at > now()
             LIMIT 1",
        )
        .bind(actor_id)
        .bind(purpose)
        .bind(code_hash)
        .fetch_optional(&self.pool)
        .await?;
        let found = matched.is_some();

        // 一致・不一致にかかわらず、この actor_id + purpose の未消費コードは全て削除する
        // （一致した場合はワンタイム消費、不一致の場合も古いコードの再利用防止）。
        sqlx::query("DELETE FROM email_short_codes WHERE actor_id = $1 AND purpose = $2")
            .bind(actor_id)
            .bind(purpose)
            .execute(&self.pool)
            .await?;

        Ok(found)
    }
}

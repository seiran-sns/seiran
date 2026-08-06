//! `auth_attempt_log` / `auth_ip_blocks` テーブルへのアクセス（#223 認証ブルートフォース対策）。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

pub struct IpBlockRow {
    pub ip_address: String,
    pub blocked_until: DateTime<Utc>,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait AuthRateLimitRepository: Send + Sync {
    /// 試行ログを1件追加し、行IDを返す。`kind`は `"login"` / `"totp"`。
    async fn record_attempt(
        &self,
        kind: &str,
        identifier_hash: &[u8],
        secret_hash: &[u8],
        ip_address: Option<&str>,
    ) -> Result<i64, sqlx::Error>;

    /// ブルートフォース種類数制限に抵触した試行ログを「拒否」としてマークする
    /// （IP拒否回数カウントの対象にするため）。
    async fn mark_rejected(&self, id: i64) -> Result<(), sqlx::Error>;

    /// `since`以降、同一`identifier_hash`に対して記録された`secret_hash`の種類数。
    async fn count_distinct_secrets(
        &self,
        kind: &str,
        identifier_hash: &[u8],
        since: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error>;

    /// `since`以降、同一`secret_hash`に対して記録された`identifier_hash`の種類数。
    async fn count_distinct_identifiers(
        &self,
        kind: &str,
        secret_hash: &[u8],
        since: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error>;

    /// `since`以降、指定IPで発生した「拒否」件数。
    async fn count_recent_rejections_by_ip(
        &self,
        ip_address: &str,
        since: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error>;

    /// 有効な（`blocked_until`が未来の）ブロックがあればその期限を返す。
    async fn find_active_ip_block(
        &self,
        ip_address: &str,
    ) -> Result<Option<DateTime<Utc>>, sqlx::Error>;

    async fn block_ip(
        &self,
        ip_address: &str,
        blocked_until: DateTime<Utc>,
        reason: &str,
    ) -> Result<(), sqlx::Error>;

    /// 現在ブロック中（`blocked_until` > now）の一覧。管理画面表示用。
    async fn list_active_ip_blocks(&self) -> Result<Vec<IpBlockRow>, sqlx::Error>;

    async fn unblock_ip(&self, ip_address: &str) -> Result<bool, sqlx::Error>;

    async fn count_account_creations(
        &self,
        ip_address: &str,
        since: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error>;

    async fn record_account_creation(&self, ip_address: &str) -> Result<(), sqlx::Error>;
}

pub struct PgAuthRateLimitRepository {
    pool: PgPool,
}

impl PgAuthRateLimitRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AuthRateLimitRepository for PgAuthRateLimitRepository {
    async fn record_attempt(
        &self,
        kind: &str,
        identifier_hash: &[u8],
        secret_hash: &[u8],
        ip_address: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO auth_attempt_log (kind, identifier_hash, secret_hash, ip_address)
             VALUES ($1, $2, $3, $4::inet)
             RETURNING id",
        )
        .bind(kind)
        .bind(identifier_hash)
        .bind(secret_hash)
        .bind(ip_address)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    async fn mark_rejected(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE auth_attempt_log SET rejected = TRUE WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn count_distinct_secrets(
        &self,
        kind: &str,
        identifier_hash: &[u8],
        since: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(DISTINCT secret_hash) FROM auth_attempt_log
             WHERE kind = $1 AND identifier_hash = $2 AND created_at >= $3",
        )
        .bind(kind)
        .bind(identifier_hash)
        .bind(since)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    async fn count_distinct_identifiers(
        &self,
        kind: &str,
        secret_hash: &[u8],
        since: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(DISTINCT identifier_hash) FROM auth_attempt_log
             WHERE kind = $1 AND secret_hash = $2 AND created_at >= $3",
        )
        .bind(kind)
        .bind(secret_hash)
        .bind(since)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    async fn count_recent_rejections_by_ip(
        &self,
        ip_address: &str,
        since: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM auth_attempt_log
             WHERE ip_address = $1::inet AND rejected = TRUE AND created_at >= $2",
        )
        .bind(ip_address)
        .bind(since)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    async fn find_active_ip_block(
        &self,
        ip_address: &str,
    ) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
        let row: Option<(DateTime<Utc>,)> = sqlx::query_as(
            "SELECT blocked_until FROM auth_ip_blocks
             WHERE ip_address = $1::inet AND blocked_until > NOW()",
        )
        .bind(ip_address)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(t,)| t))
    }

    async fn block_ip(
        &self,
        ip_address: &str,
        blocked_until: DateTime<Utc>,
        reason: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO auth_ip_blocks (ip_address, blocked_until, reason)
             VALUES ($1::inet, $2, $3)
             ON CONFLICT (ip_address) DO UPDATE
               SET blocked_until = EXCLUDED.blocked_until,
                   reason = EXCLUDED.reason",
        )
        .bind(ip_address)
        .bind(blocked_until)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_active_ip_blocks(&self) -> Result<Vec<IpBlockRow>, sqlx::Error> {
        let rows: Vec<(String, DateTime<Utc>, String, DateTime<Utc>)> = sqlx::query_as(
            "SELECT ip_address::text, blocked_until, reason, created_at FROM auth_ip_blocks
             WHERE blocked_until > NOW()
             ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(ip_address, blocked_until, reason, created_at)| IpBlockRow {
                    ip_address,
                    blocked_until,
                    reason,
                    created_at,
                },
            )
            .collect())
    }

    async fn unblock_ip(&self, ip_address: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM auth_ip_blocks WHERE ip_address = $1::inet")
            .bind(ip_address)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn count_account_creations(
        &self,
        ip_address: &str,
        since: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM account_creation_log
             WHERE ip_address = $1::inet AND created_at >= $2",
        )
        .bind(ip_address)
        .bind(since)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    async fn record_account_creation(&self, ip_address: &str) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO account_creation_log (ip_address) VALUES ($1::inet)")
            .bind(ip_address)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

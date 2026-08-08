use async_trait::async_trait;
use sqlx::PgPool;

/// `confirm` の結果。`Confirmed` はこの呼び出しで初めて確定したことを、
/// `AlreadyConfirmed` は既に確定済み（レース時は他プロセスが先に確定していた）ことを表す。
/// いずれの場合も `String` は実際にDBへ確定している値。
pub enum ConfirmOutcome {
    Confirmed(String),
    AlreadyConfirmed(String),
}

/// 自ホストドメインの確定値を扱う。`instance_domain` テーブルは単一行のみ許容し、
/// このトレイトに `UPDATE` に相当するメソッドは存在しない（一度確定したら不変）。
#[async_trait]
pub trait InstanceDomainRepository: Send + Sync {
    async fn get(&self) -> Result<Option<String>, sqlx::Error>;

    /// 冪等な確定操作。未確定なら `domain` で確定し `Confirmed` を返す。
    /// 既に確定済みなら（`domain` の値によらず）既存の確定値で `AlreadyConfirmed` を返す。
    async fn confirm(&self, domain: &str) -> Result<ConfirmOutcome, sqlx::Error>;
}

pub struct PgInstanceDomainRepository {
    pool: PgPool,
}

impl PgInstanceDomainRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl InstanceDomainRepository for PgInstanceDomainRepository {
    async fn get(&self) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as("SELECT domain FROM instance_domain")
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(v,)| v))
    }

    async fn confirm(&self, domain: &str) -> Result<ConfirmOutcome, sqlx::Error> {
        let inserted: Option<(String,)> = sqlx::query_as(
            "INSERT INTO instance_domain (id, domain) VALUES (1, $1)
             ON CONFLICT (id) DO NOTHING
             RETURNING domain",
        )
        .bind(domain)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((confirmed,)) = inserted {
            return Ok(ConfirmOutcome::Confirmed(confirmed));
        }

        // ON CONFLICTでスキップされた = 既に確定済み。実際の確定値を読み直して返す。
        let (existing,): (String,) = sqlx::query_as("SELECT domain FROM instance_domain")
            .fetch_one(&self.pool)
            .await?;
        Ok(ConfirmOutcome::AlreadyConfirmed(existing))
    }
}

//! プロフィールの「別のアカウント」機能（alsoKnownAs）の永続化。
//!
//! Mastodon/MisskeyのAP `alsoKnownAs`は本来アカウント引っ越し（Move）専用のフィールド
//! だが、seiranではそれとは独立に「同一人物が持つ複数アカウントの相互リンク表示」の
//! ために転用する（`docs/protocols.md`参照）。`owner_actor_id`が「`target_actor_id`も
//! 自分だ」と申告する片方向の関係で、相手側（fedi/ローカルのみ、bskyは対象外）も
//! 逆向きに同じ申告をしていれば相互検証済み（`verified`）とみなす。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// `owner_actor_id`の登録一覧1行（アクター表示情報 + 検証結果込み）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AlsoKnownAsRow {
    pub target_actor_id: i64,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub actor_type: String,
    pub avatar_url: Option<String>,
    /// 自分のAP actor文書へ`alsoKnownAs`として公開する際、fediターゲットのURIに使う。
    pub ap_uri: Option<String>,
    /// 同上、bskyターゲットの`did:...`形式URIに使う。
    pub at_did: Option<String>,
    /// 相手側も逆向きにこちらを`also_known_as`として指定しているか（非同期ジョブがキャッシュ）。
    pub verified: bool,
    /// 直近の検証実行時刻。`None`はまだ一度も検証されていない。
    pub last_checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait AlsoKnownAsRepository: Send + Sync {
    /// エントリを追加する（既に追加済みなら何もしない）。
    async fn add(
        &self,
        owner_actor_id: i64,
        target_actor_id: i64,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// エントリを削除する。削除できたら `true`。
    async fn remove(&self, owner_actor_id: i64, target_actor_id: i64) -> Result<bool, sqlx::Error>;

    /// `owner_actor_id`の登録一覧（アクター情報込み、登録日時降順）。
    async fn list_with_actor_info(
        &self,
        owner_actor_id: i64,
    ) -> Result<Vec<AlsoKnownAsRow>, sqlx::Error>;

    /// `owner_actor_id`の登録件数（上限チェック用）。
    async fn count_by_owner(&self, owner_actor_id: i64) -> Result<i64, sqlx::Error>;

    /// `target_actor_id`（ローカルアクター限定）が`owner_actor_id`を自分の
    /// also_known_asとして逆向きに登録済みか（ローカル同士の相互検証はAP取得不要のため
    /// DB直接参照で完結させる）。
    async fn is_listed_by(
        &self,
        target_actor_id: i64,
        owner_actor_id: i64,
    ) -> Result<bool, sqlx::Error>;

    /// 検証結果（`verified`/`last_checked_at`）を更新する。
    async fn set_verification(
        &self,
        owner_actor_id: i64,
        target_actor_id: i64,
        verified: bool,
        checked_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// リモートFediアクター（`owner_actor_id`）自身のAP actor文書が公開している
    /// `alsoKnownAs`を`target_actor_ids`として同期する（`jobs::also_known_as_sync`が使う）。
    /// 最新の集合に無いエントリを削除し、新規エントリのみ追加する（既存エントリは
    /// `verified`/`last_checked_at`を保持したまま残す。`ON CONFLICT DO NOTHING`で無視）。
    async fn sync_remote_owner_targets(
        &self,
        owner_actor_id: i64,
        target_actor_ids: &[i64],
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;
}

pub struct PgAlsoKnownAsRepository {
    pool: PgPool,
}

impl PgAlsoKnownAsRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AlsoKnownAsRepository for PgAlsoKnownAsRepository {
    async fn add(
        &self,
        owner_actor_id: i64,
        target_actor_id: i64,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO actor_also_known_as (owner_actor_id, target_actor_id, created_at)
             VALUES ($1, $2, $3)
             ON CONFLICT (owner_actor_id, target_actor_id) DO NOTHING",
        )
        .bind(owner_actor_id)
        .bind(target_actor_id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn remove(&self, owner_actor_id: i64, target_actor_id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "DELETE FROM actor_also_known_as WHERE owner_actor_id = $1 AND target_actor_id = $2",
        )
        .bind(owner_actor_id)
        .bind(target_actor_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_with_actor_info(
        &self,
        owner_actor_id: i64,
    ) -> Result<Vec<AlsoKnownAsRow>, sqlx::Error> {
        sqlx::query_as::<_, AlsoKnownAsRow>(
            "SELECT a.id AS target_actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type,
                    COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url,
                    a.ap_uri, a.at_did,
                    aka.verified, aka.last_checked_at, aka.created_at
             FROM actor_also_known_as aka
             JOIN actors a ON a.id = aka.target_actor_id
             LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
             LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
             WHERE aka.owner_actor_id = $1
             ORDER BY aka.created_at DESC",
        )
        .bind(owner_actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn count_by_owner(&self, owner_actor_id: i64) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM actor_also_known_as WHERE owner_actor_id = $1")
            .bind(owner_actor_id)
            .fetch_one(&self.pool)
            .await
    }

    async fn is_listed_by(
        &self,
        target_actor_id: i64,
        owner_actor_id: i64,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM actor_also_known_as WHERE owner_actor_id = $1 AND target_actor_id = $2)",
        )
        .bind(target_actor_id)
        .bind(owner_actor_id)
        .fetch_one(&self.pool)
        .await
    }

    async fn set_verification(
        &self,
        owner_actor_id: i64,
        target_actor_id: i64,
        verified: bool,
        checked_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE actor_also_known_as SET verified = $3, last_checked_at = $4
             WHERE owner_actor_id = $1 AND target_actor_id = $2",
        )
        .bind(owner_actor_id)
        .bind(target_actor_id)
        .bind(verified)
        .bind(checked_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn sync_remote_owner_targets(
        &self,
        owner_actor_id: i64,
        target_actor_ids: &[i64],
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "DELETE FROM actor_also_known_as
             WHERE owner_actor_id = $1 AND NOT (target_actor_id = ANY($2))",
        )
        .bind(owner_actor_id)
        .bind(target_actor_ids)
        .execute(&self.pool)
        .await?;

        if !target_actor_ids.is_empty() {
            sqlx::query(
                "INSERT INTO actor_also_known_as (owner_actor_id, target_actor_id, created_at)
                 SELECT $1, t, $3 FROM unnest($2::bigint[]) AS t
                 ON CONFLICT (owner_actor_id, target_actor_id) DO NOTHING",
            )
            .bind(owner_actor_id)
            .bind(target_actor_ids)
            .bind(now)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }
}

use std::collections::HashSet;

use async_trait::async_trait;
use sqlx::PgPool;

/// リポストミュート中アクターの表示用情報（設定画面のミュート・ブロック一覧）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RepostMutedActorRow {
    pub id: i64,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

#[async_trait]
pub trait RepostMuteRepository: Send + Sync {
    /// リポストミュートを挿入する（既存なら何もしない）。AP/ATP 配送は発生しないローカル効果のみ。
    async fn insert(&self, muter_actor_id: i64, muted_actor_id: i64) -> Result<(), sqlx::Error>;

    async fn delete_by_actors(
        &self,
        muter_actor_id: i64,
        muted_actor_id: i64,
    ) -> Result<(), sqlx::Error>;

    async fn is_muted(&self, muter_actor_id: i64, muted_actor_id: i64)
        -> Result<bool, sqlx::Error>;

    /// リポストミュート中のアクター一覧を新しい順に返す（設定画面）。件数は少数想定のため
    /// カーソルページネーションはせず先頭200件を返す。
    async fn list_muted(
        &self,
        muter_actor_id: i64,
    ) -> Result<Vec<RepostMutedActorRow>, sqlx::Error>;

    /// `candidate_ids` のうち muter_actor_id がリポストミュート中のものだけを返す
    /// （タイムラインのper-note relationship付与でのN+1回避用）。
    async fn list_muted_among(
        &self,
        muter_actor_id: i64,
        candidate_ids: &[i64],
    ) -> Result<HashSet<i64>, sqlx::Error>;
}

pub struct PgRepostMuteRepository {
    pool: PgPool,
}

impl PgRepostMuteRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl RepostMuteRepository for PgRepostMuteRepository {
    async fn insert(&self, muter_actor_id: i64, muted_actor_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO repost_mutes (muter_actor_id, muted_actor_id)
             VALUES ($1, $2)
             ON CONFLICT (muter_actor_id, muted_actor_id) DO NOTHING",
        )
        .bind(muter_actor_id)
        .bind(muted_actor_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn delete_by_actors(
        &self,
        muter_actor_id: i64,
        muted_actor_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM repost_mutes WHERE muter_actor_id = $1 AND muted_actor_id = $2")
            .bind(muter_actor_id)
            .bind(muted_actor_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn is_muted(
        &self,
        muter_actor_id: i64,
        muted_actor_id: i64,
    ) -> Result<bool, sqlx::Error> {
        let row: (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM repost_mutes WHERE muter_actor_id = $1 AND muted_actor_id = $2)",
        )
        .bind(muter_actor_id)
        .bind(muted_actor_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    async fn list_muted(
        &self,
        muter_actor_id: i64,
    ) -> Result<Vec<RepostMutedActorRow>, sqlx::Error> {
        sqlx::query_as::<_, RepostMutedActorRow>(
            "SELECT a.id, a.username, a.domain, a.display_name,
                    COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url
             FROM repost_mutes m
             JOIN actors a ON a.id = m.muted_actor_id
             LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
             LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
             WHERE m.muter_actor_id = $1 AND a.withdrawn_at IS NULL
             ORDER BY m.id DESC
             LIMIT 200",
        )
        .bind(muter_actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn list_muted_among(
        &self,
        muter_actor_id: i64,
        candidate_ids: &[i64],
    ) -> Result<HashSet<i64>, sqlx::Error> {
        let rows: Vec<i64> = sqlx::query_scalar(
            "SELECT muted_actor_id FROM repost_mutes WHERE muter_actor_id = $1 AND muted_actor_id = ANY($2)",
        )
        .bind(muter_actor_id)
        .bind(candidate_ids)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().collect())
    }
}

use async_trait::async_trait;
use sqlx::PgPool;

/// ミュート中アクターの表示用情報（設定画面のミュート一覧、#55）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MutedActorRow {
    pub id: i64,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

#[async_trait]
pub trait MuteRepository: Send + Sync {
    /// ミュートを挿入する（既存なら何もしない）。AP/ATP 配送は発生しないローカル効果のみ。
    async fn insert(&self, muter_actor_id: i64, muted_actor_id: i64) -> Result<(), sqlx::Error>;

    async fn delete_by_actors(
        &self,
        muter_actor_id: i64,
        muted_actor_id: i64,
    ) -> Result<(), sqlx::Error>;

    async fn is_muted(&self, muter_actor_id: i64, muted_actor_id: i64) -> Result<bool, sqlx::Error>;

    /// ミュート中のアクター一覧を新しい順に返す（設定画面、#55）。件数は少数想定のため
    /// カーソルページネーションはせず先頭200件を返す。
    async fn list_muted(&self, muter_actor_id: i64) -> Result<Vec<MutedActorRow>, sqlx::Error>;
}

pub struct PgMuteRepository {
    pool: PgPool,
}

impl PgMuteRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MuteRepository for PgMuteRepository {
    async fn insert(&self, muter_actor_id: i64, muted_actor_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO mutes (muter_actor_id, muted_actor_id)
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
        sqlx::query(
            "DELETE FROM mutes WHERE muter_actor_id = $1 AND muted_actor_id = $2",
        )
        .bind(muter_actor_id)
        .bind(muted_actor_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn is_muted(&self, muter_actor_id: i64, muted_actor_id: i64) -> Result<bool, sqlx::Error> {
        let row: (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM mutes WHERE muter_actor_id = $1 AND muted_actor_id = $2)",
        )
        .bind(muter_actor_id)
        .bind(muted_actor_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    async fn list_muted(&self, muter_actor_id: i64) -> Result<Vec<MutedActorRow>, sqlx::Error> {
        sqlx::query_as::<_, MutedActorRow>(
            "SELECT a.id, a.username, a.domain, a.display_name,
                    COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url
             FROM mutes m
             JOIN actors a ON a.id = m.muted_actor_id
             LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
             LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
             WHERE m.muter_actor_id = $1
             ORDER BY m.id DESC
             LIMIT 200",
        )
        .bind(muter_actor_id)
        .fetch_all(&self.pool)
        .await
    }
}

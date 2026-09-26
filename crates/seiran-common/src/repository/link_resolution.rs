//! bio/profile_fields中のURL（Fedi/Bsky）を非同期解決した結果のキャッシュ（`link_resolutions`）。
//! `jobs::link_resolve`が書き込み、プロフィール取得API（`seiran-api`）が読み出す。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LinkResolutionRow {
    pub url: String,
    /// `"actor"` | `"post"` | `"none"`（陰性）。
    pub kind: String,
    pub resolved_actor_id: Option<i64>,
    pub resolved_post_id: Option<i64>,
    pub checked_at: DateTime<Utc>,
}

#[async_trait]
pub trait LinkResolutionRepository: Send + Sync {
    /// 指定URL群のキャッシュ済み行をまとめて取得する（N+1回避）。キャッシュに無いURLは
    /// 戻り値に含まれない（呼び出し側は「未解決」として非同期ジョブをenqueueする）。
    async fn find_by_urls(&self, urls: &[String]) -> Result<Vec<LinkResolutionRow>, sqlx::Error>;

    /// 解決結果（陽性・陰性いずれも）を書き込む（初回・再調査のどちらも同じUPSERT）。
    async fn upsert(
        &self,
        url: &str,
        kind: &str,
        resolved_actor_id: Option<i64>,
        resolved_post_id: Option<i64>,
    ) -> Result<(), sqlx::Error>;
}

pub struct PgLinkResolutionRepository {
    pool: PgPool,
}

impl PgLinkResolutionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl LinkResolutionRepository for PgLinkResolutionRepository {
    async fn find_by_urls(&self, urls: &[String]) -> Result<Vec<LinkResolutionRow>, sqlx::Error> {
        if urls.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_as::<_, LinkResolutionRow>(
            "SELECT url, kind, resolved_actor_id, resolved_post_id, checked_at
             FROM link_resolutions
             WHERE url = ANY($1)",
        )
        .bind(urls)
        .fetch_all(&self.pool)
        .await
    }

    async fn upsert(
        &self,
        url: &str,
        kind: &str,
        resolved_actor_id: Option<i64>,
        resolved_post_id: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO link_resolutions (url, kind, resolved_actor_id, resolved_post_id, checked_at)
             VALUES ($1, $2, $3, $4, now())
             ON CONFLICT (url) DO UPDATE SET
                 kind = EXCLUDED.kind,
                 resolved_actor_id = EXCLUDED.resolved_actor_id,
                 resolved_post_id = EXCLUDED.resolved_post_id,
                 checked_at = EXCLUDED.checked_at",
        )
        .bind(url)
        .bind(kind)
        .bind(resolved_actor_id)
        .bind(resolved_post_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }
}

//! リモートサーバー由来のカスタム絵文字カタログ（`remote_emojis`）(#73)。
//!
//! AP受信（投稿本文・表示名・絵文字リアクション）で見つけたカスタム絵文字を
//! `upsert_seen` で記録し、管理画面の「リモート」タブ一覧・NoteCard右クリック
//! インポート導線の両方から参照される。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RemoteEmojiRow {
    pub id: i64,
    pub shortcode: String,
    pub domain: String,
    pub image_url: String,
    pub tags: Vec<String>,
    pub license: Option<String>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

#[async_trait]
pub trait RemoteEmojiRepository: Send + Sync {
    /// `shortcode`/`domain` の部分一致で絞り込んで一覧を返す（`keyword` が空なら全件）。
    /// 直近に見つかったもの順（`last_seen_at` 降順）。
    async fn list(&self, keyword: Option<&str>) -> Result<Vec<RemoteEmojiRow>, sqlx::Error>;

    /// AP受信で見つけたカスタム絵文字を記録する（`shortcode`+`domain` で upsert）。
    /// 既存行があれば `image_url`/`last_seen_at` を更新し、`license` は既知の値を保持する
    /// （AP標準の Emoji タグにライセンス情報は乗らないため通常 `None` のまま）。
    async fn upsert_seen(
        &self,
        shortcode: &str,
        domain: &str,
        image_url: &str,
        tags: &[String],
        license: Option<&str>,
    ) -> Result<(), sqlx::Error>;
}

pub struct PgRemoteEmojiRepository {
    pool: PgPool,
}

impl PgRemoteEmojiRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl RemoteEmojiRepository for PgRemoteEmojiRepository {
    async fn list(&self, keyword: Option<&str>) -> Result<Vec<RemoteEmojiRow>, sqlx::Error> {
        let pattern = keyword.map(|k| format!("%{}%", k));
        sqlx::query_as::<_, RemoteEmojiRow>(
            "SELECT id, shortcode, domain, image_url, tags, license, first_seen_at, last_seen_at
             FROM remote_emojis
             WHERE $1::text IS NULL
                OR shortcode ILIKE $1
                OR EXISTS (SELECT 1 FROM unnest(tags) AS tag WHERE tag ILIKE $1)
             ORDER BY last_seen_at DESC",
        )
        .bind(pattern)
        .fetch_all(&self.pool)
        .await
    }

    async fn upsert_seen(
        &self,
        shortcode: &str,
        domain: &str,
        image_url: &str,
        tags: &[String],
        license: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();
        let id = crate::generate_snowflake_id(now);
        sqlx::query(
            "INSERT INTO remote_emojis (id, shortcode, domain, image_url, tags, license, first_seen_at, last_seen_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $7)
             ON CONFLICT (shortcode, domain) DO UPDATE
                 SET image_url = excluded.image_url,
                     tags = CASE WHEN cardinality(excluded.tags) > 0 THEN excluded.tags ELSE remote_emojis.tags END,
                     license = COALESCE(excluded.license, remote_emojis.license),
                     last_seen_at = excluded.last_seen_at",
        )
        .bind(id)
        .bind(shortcode)
        .bind(domain)
        .bind(image_url)
        .bind(tags)
        .bind(license)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

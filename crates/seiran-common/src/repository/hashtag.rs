use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use super::TimelinePost;
use crate::hashtag::extract_hashtags;

/// ピン留めされたハッシュタグの1行（ホーム画面タブ表示用）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PinnedHashtagRow {
    pub name: String,
    pub pinned_at: DateTime<Utc>,
}

#[async_trait]
pub trait HashtagRepository: Send + Sync {
    /// `text` からハッシュタグを抽出し、`hashtags` に upsert、`post_hashtags` にリンクする。
    /// 抽出結果が空なら何もしない。ローカル投稿・AP受信・Bsky受信の投稿INSERT直後から
    /// 呼ばれる共通口（3プロトコル共通の抽出経路、`docs/protocols.md` 6節参照）。
    async fn link_post(&self, post_id: i64, text: &str) -> Result<(), sqlx::Error>;

    /// ハッシュタイムライン（正規化済み `tag_name` で検索、`posts.id` 降順）。
    /// 可視性は public/unlisted のみを対象にする（特定アクター向けの閲覧制御が要る
    /// フィードではなく発見用の公開フィードのため、followers_only の例外は設けない）。
    async fn timeline(
        &self,
        tag_name: &str,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
        viewer_actor_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// ホーム画面タブへのピン留め（既に存在するタグ名を渡してもよい。無ければ作成する）。
    async fn pin(&self, actor_id: i64, tag_name: &str, now: DateTime<Utc>) -> Result<(), sqlx::Error>;

    /// ピン留め解除。解除できたら `true`。
    async fn unpin(&self, actor_id: i64, tag_name: &str) -> Result<bool, sqlx::Error>;

    /// actor がピン留めしたハッシュタグ一覧（`pinned_at` 降順）。ハッシュタグ画面の
    /// ボタン状態判定（フロント側で一覧に含まれるか見る）にも流用する。
    async fn list_pinned(&self, actor_id: i64) -> Result<Vec<PinnedHashtagRow>, sqlx::Error>;
}

pub struct PgHashtagRepository {
    pool: PgPool,
}

impl PgHashtagRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl HashtagRepository for PgHashtagRepository {
    async fn link_post(&self, post_id: i64, text: &str) -> Result<(), sqlx::Error> {
        let names = extract_hashtags(text);
        if names.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for name in &names {
            // ON CONFLICT DO NOTHING だと既存行の id が RETURNING に乗らないため、
            // 実質的な no-op 更新にして常に id を取れるようにする。
            let hashtag_id: i64 = sqlx::query_scalar(
                "INSERT INTO hashtags (name) VALUES ($1)
                 ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name
                 RETURNING id",
            )
            .bind(name)
            .fetch_one(&mut *tx)
            .await?;

            sqlx::query(
                "INSERT INTO post_hashtags (post_id, hashtag_id) VALUES ($1, $2)
                 ON CONFLICT DO NOTHING",
            )
            .bind(post_id)
            .bind(hashtag_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await
    }

    async fn timeline(
        &self,
        tag_name: &str,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
        viewer_actor_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets
             FROM post_hashtags ph
             JOIN hashtags h ON h.id = ph.hashtag_id
             JOIN posts p ON p.id = ph.post_id
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE h.name = $1 AND p.deleted_at IS NULL
               AND p.visibility IN ('public', 'unlisted')
               AND ($2::bigint IS NULL OR p.id < $2)
               AND ($3::bigint IS NULL OR p.id > $3)
               AND ($5::bigint IS NULL OR p.actor_id = $5 OR NOT actor_is_hidden_for_viewer($5, p.actor_id))
             ORDER BY p.id DESC LIMIT $4",
        )
        .bind(tag_name)
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .bind(viewer_actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn pin(&self, actor_id: i64, tag_name: &str, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        let hashtag_id: i64 = sqlx::query_scalar(
            "INSERT INTO hashtags (name) VALUES ($1)
             ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name
             RETURNING id",
        )
        .bind(tag_name)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO pinned_hashtags (actor_id, hashtag_id, pinned_at) VALUES ($1, $2, $3)
             ON CONFLICT (actor_id, hashtag_id) DO NOTHING",
        )
        .bind(actor_id)
        .bind(hashtag_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await
    }

    async fn unpin(&self, actor_id: i64, tag_name: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "DELETE FROM pinned_hashtags
             WHERE actor_id = $1 AND hashtag_id = (SELECT id FROM hashtags WHERE name = $2)",
        )
        .bind(actor_id)
        .bind(tag_name)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_pinned(&self, actor_id: i64) -> Result<Vec<PinnedHashtagRow>, sqlx::Error> {
        sqlx::query_as::<_, PinnedHashtagRow>(
            "SELECT h.name, ph.pinned_at
             FROM pinned_hashtags ph
             JOIN hashtags h ON h.id = ph.hashtag_id
             WHERE ph.actor_id = $1
             ORDER BY ph.pinned_at DESC",
        )
        .bind(actor_id)
        .fetch_all(&self.pool)
        .await
    }
}

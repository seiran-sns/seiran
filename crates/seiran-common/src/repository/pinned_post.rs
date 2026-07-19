use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use super::TimelinePost;

/// actor がピン留めできる最大件数。超過分は最古（`pinned_at` が最も古いもの）から自動的に外れる。
pub const MAX_PINNED_POSTS: i64 = 5;

#[async_trait]
pub trait PinnedPostsRepository: Send + Sync {
    /// ピン留めを追加する。既に `MAX_PINNED_POSTS` 件ある場合は最古の分を追い出す。
    /// 追い出された `post_id` を返す（無ければ空）。ATP の Bsky プロフィール再同期の
    /// トリガー判定に使う。
    async fn pin(&self, actor_id: i64, post_id: i64) -> Result<Vec<i64>, sqlx::Error>;

    /// ピン留めを解除する。削除できた場合は `true`。
    async fn unpin(&self, actor_id: i64, post_id: i64) -> Result<bool, sqlx::Error>;

    /// actor のピン留め post_id 一覧（`pinned_at` 降順、最新のピン留めが先頭）。
    async fn list_by_actor(&self, actor_id: i64) -> Result<Vec<i64>, sqlx::Error>;

    /// actor のピン留め投稿を、タイムラインと同じ結合行（アクター情報込み）で取得する（`pinned_at` 降順）。
    /// `viewer_actor_id` は閲覧者の actor_id（匿名なら `None`）。`followers_only`/`direct` は
    /// 投稿者本人または accepted フォロワーの閲覧者にのみ返す（可視性による閲覧制御）。
    async fn list_timeline_by_actor(&self, actor_id: i64, viewer_actor_id: Option<i64>) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// リモートアクター（Fedi の featured collection / Bsky の `pinnedPost`）から取得した
    /// 最新のピン留め状態でこのテーブルを洗い替える。`post_ids` は同期元での並び順
    /// （先頭ほど優先度が高い）。既存行のうち `post_ids` に無いものは削除し、
    /// 無い分は追加する。`now` を基準に、並び順を保つよう `pinned_at` を割り振る。
    async fn sync_from_remote(
        &self,
        actor_id: i64,
        post_ids: &[i64],
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;
}

pub struct PgPinnedPostsRepository {
    pool: PgPool,
}

impl PgPinnedPostsRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PinnedPostsRepository for PgPinnedPostsRepository {
    async fn pin(&self, actor_id: i64, post_id: i64) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query(
            "INSERT INTO pinned_posts (actor_id, post_id) VALUES ($1, $2)
             ON CONFLICT (actor_id, post_id) DO NOTHING",
        )
        .bind(actor_id)
        .bind(post_id)
        .execute(&self.pool)
        .await?;

        sqlx::query_scalar::<_, i64>(
            "DELETE FROM pinned_posts WHERE id IN (
                SELECT id FROM pinned_posts WHERE actor_id = $1
                ORDER BY pinned_at DESC OFFSET $2
             ) RETURNING post_id",
        )
        .bind(actor_id)
        .bind(MAX_PINNED_POSTS)
        .fetch_all(&self.pool)
        .await
    }

    async fn unpin(&self, actor_id: i64, post_id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM pinned_posts WHERE actor_id = $1 AND post_id = $2")
            .bind(actor_id)
            .bind(post_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_by_actor(&self, actor_id: i64) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT post_id FROM pinned_posts WHERE actor_id = $1 ORDER BY pinned_at DESC",
        )
        .bind(actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn list_timeline_by_actor(&self, actor_id: i64, viewer_actor_id: Option<i64>) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, p.actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets
             FROM pinned_posts pp
             JOIN posts p ON p.id = pp.post_id
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE pp.actor_id = $1 AND p.deleted_at IS NULL
               AND (
                   p.visibility NOT IN ('followers_only', 'direct')
                   OR p.actor_id = $2
                   OR EXISTS (
                       SELECT 1 FROM follows f
                       WHERE f.follower_actor_id = $2 AND f.target_actor_id = p.actor_id AND f.status = 'accepted'
                   )
               )
             ORDER BY pp.pinned_at DESC",
        )
        .bind(actor_id)
        .bind(viewer_actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn sync_from_remote(
        &self,
        actor_id: i64,
        post_ids: &[i64],
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM pinned_posts WHERE actor_id = $1 AND NOT (post_id = ANY($2))")
            .bind(actor_id)
            .bind(post_ids)
            .execute(&mut *tx)
            .await?;

        // 同期元での並び順（先頭ほど優先）を pinned_at の新しい順に対応させる。
        for (idx, post_id) in post_ids.iter().enumerate() {
            let pinned_at = now - chrono::Duration::milliseconds(idx as i64);
            sqlx::query(
                "INSERT INTO pinned_posts (actor_id, post_id, pinned_at) VALUES ($1, $2, $3)
                 ON CONFLICT (actor_id, post_id) DO NOTHING",
            )
            .bind(actor_id)
            .bind(post_id)
            .bind(pinned_at)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await
    }
}

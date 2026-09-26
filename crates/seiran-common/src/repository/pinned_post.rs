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
    /// `viewer_actor_id` は閲覧者の actor_id（匿名なら `None`）。可視性は他の取得経路と同じ
    /// `post_is_visible_to` で判定する。
    async fn list_timeline_by_actor(
        &self,
        actor_id: i64,
        viewer_actor_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

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
        // 追加と上限超過分の削除を1トランザクションにし、同じアクターの同時ピン留めは
        // アクター行のロックで直列化する（別々の文だと同時実行で上限を超えて残りうる）。
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT 1 FROM actors WHERE id = $1 FOR UPDATE")
            .bind(actor_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO pinned_posts (actor_id, post_id) VALUES ($1, $2)
             ON CONFLICT (actor_id, post_id) DO NOTHING",
        )
        .bind(actor_id)
        .bind(post_id)
        .execute(&mut *tx)
        .await?;

        let removed = sqlx::query_scalar::<_, i64>(
            "DELETE FROM pinned_posts pp
             USING (
                 SELECT id FROM pinned_posts WHERE actor_id = $1
                 ORDER BY pinned_at DESC OFFSET $2
             ) overflow
             WHERE pp.id = overflow.id
             RETURNING pp.post_id",
        )
        .bind(actor_id)
        .bind(MAX_PINNED_POSTS)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(removed)
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

    async fn list_timeline_by_actor(
        &self,
        actor_id: i64,
        viewer_actor_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            concat!("SELECT ", crate::timeline_post_columns!(), "
             FROM pinned_posts pp
             JOIN posts p ON p.id = pp.post_id
             ", crate::timeline_post_joins!(), "
             WHERE pp.actor_id = $1 AND p.deleted_at IS NULL
               AND ($2::bigint IS NULL OR p.actor_id = $2 OR NOT actor_is_hidden_for_viewer($2, p.actor_id))
               AND post_is_visible_to($2, p.actor_id, p.visibility::text, p.id, false)
             ORDER BY pp.pinned_at DESC"),
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

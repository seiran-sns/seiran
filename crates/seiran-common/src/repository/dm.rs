//! ダイレクトメッセージ（`visibility='direct'`投稿のスレッド管理・既読状態）。
//!
//! 投稿本体（作成・タイムライン除外）は `post.rs` の `PostRepository` が担う。
//! ここでは「スレッド起点を同じくするdirect投稿の集合」をメッセージセッションとして
//! 扱うための一覧・履歴・既読状態のクエリのみを持つ。

use async_trait::async_trait;
use sqlx::PgPool;

pub use super::post::{DmSessionSummary, TimelinePost};

/// DMセッション一覧の相手表示用アクター要約（`actor_search`と同じavatar_url解決を使う）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DmPeerSummary {
    pub id: i64,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub actor_type: String,
    pub avatar_url: Option<String>,
}

#[async_trait]
pub trait DmRepository: Send + Sync {
    /// 自分が参加している（投稿者 or 宛先）DMセッション一覧を、最終メッセージのid降順で
    /// カーソルページネーション取得する。
    async fn sessions(
        &self,
        actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<DmSessionSummary>, sqlx::Error>;

    /// 指定スレッド起点のメッセージ履歴を時刻順（id昇順、最下部が最新）で取得する。
    /// 呼び出し元が別途 `is_participant` で閲覧権限を確認すること（このメソッド自体は
    /// 権限チェックを行わない）。
    async fn thread_messages(
        &self,
        thread_root_post_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// 指定スレッドの最新（id最大）ポストIDを取得する。既読カーソル更新に使う。
    async fn latest_post_id(&self, thread_root_post_id: i64) -> Result<Option<i64>, sqlx::Error>;

    /// 指定アクターが指定スレッドの参加者（投稿者 or 宛先のいずれか）かどうかを判定する。
    async fn is_participant(&self, thread_root_post_id: i64, actor_id: i64) -> Result<bool, sqlx::Error>;

    /// スレッドの最終既読ポストIDを記録する（`last_read_post_id`は単調増加のみ許可）。
    async fn mark_read(&self, actor_id: i64, thread_root_post_id: i64, last_read_post_id: i64) -> Result<(), sqlx::Error>;

    /// 未読のあるセッション数（バッジ表示用）。
    async fn unread_session_count(&self, actor_id: i64) -> Result<i64, sqlx::Error>;

    /// 複数スレッドの最終既読ポストIDを一括取得する（セッション一覧の未読フラグ算出用）。
    /// 戻り値は `(thread_root_post_id, last_read_post_id)` のタプル列（未読状態が無いスレッドは含まれない）。
    async fn read_states(&self, actor_id: i64, thread_root_post_ids: &[i64]) -> Result<Vec<(i64, i64)>, sqlx::Error>;

    /// 投稿の宛先アクターID一覧を取得する（AP配送のto/cc組み立て用）。
    async fn recipient_ids(&self, post_id: i64) -> Result<Vec<i64>, sqlx::Error>;

    /// セッション一覧の相手表示用に、複数アクターIDの要約情報を一括取得する。
    async fn peer_summaries(&self, actor_ids: &[i64]) -> Result<Vec<DmPeerSummary>, sqlx::Error>;
}

pub struct PgDmRepository {
    pool: PgPool,
}

impl PgDmRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl DmRepository for PgDmRepository {
    async fn sessions(
        &self,
        actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<DmSessionSummary>, sqlx::Error> {
        sqlx::query_as::<_, DmSessionSummary>(
            "WITH my_threads AS (
                 SELECT DISTINCT p.thread_root_post_id
                 FROM posts p
                 WHERE p.thread_root_post_id IS NOT NULL AND p.deleted_at IS NULL
                   AND (
                       p.actor_id = $1
                       OR EXISTS (SELECT 1 FROM post_recipients pr WHERE pr.post_id = p.id AND pr.actor_id = $1)
                   )
             ),
             latest AS (
                 SELECT tr.thread_root_post_id, lp.id AS last_post_id, lp.body AS last_body, lp.created_at AS last_created_at
                 FROM my_threads tr
                 JOIN LATERAL (
                     SELECT id, body, created_at FROM posts
                     WHERE thread_root_post_id = tr.thread_root_post_id AND deleted_at IS NULL
                     ORDER BY id DESC LIMIT 1
                 ) lp ON true
                 WHERE ($2::bigint IS NULL OR lp.id < $2)
                   AND ($3::bigint IS NULL OR lp.id > $3)
             ),
             peers AS (
                 SELECT p.thread_root_post_id, p.actor_id AS peer_id
                 FROM posts p
                 WHERE p.thread_root_post_id IN (SELECT thread_root_post_id FROM latest) AND p.deleted_at IS NULL
                 UNION
                 SELECT p.thread_root_post_id, pr.actor_id AS peer_id
                 FROM posts p JOIN post_recipients pr ON pr.post_id = p.id
                 WHERE p.thread_root_post_id IN (SELECT thread_root_post_id FROM latest) AND p.deleted_at IS NULL
             )
             SELECT l.thread_root_post_id, l.last_post_id, l.last_body, l.last_created_at,
                    COALESCE(array_agg(DISTINCT pe.peer_id) FILTER (WHERE pe.peer_id IS NOT NULL AND pe.peer_id != $1), ARRAY[]::bigint[]) AS peer_actor_ids
             FROM latest l
             LEFT JOIN peers pe ON pe.thread_root_post_id = l.thread_root_post_id
             GROUP BY l.thread_root_post_id, l.last_post_id, l.last_body, l.last_created_at
             ORDER BY l.last_post_id DESC
             LIMIT $4",
        )
        .bind(actor_id)
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn thread_messages(
        &self,
        thread_root_post_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets
             FROM posts p JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.thread_root_post_id = $1 AND p.deleted_at IS NULL
               AND ($3::bigint IS NULL OR p.id < $3)
               AND ($4::bigint IS NULL OR p.id > $4)
             ORDER BY p.id ASC
             LIMIT $2",
        )
        .bind(thread_root_post_id)
        .bind(limit)
        .bind(until_id)
        .bind(since_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn latest_post_id(&self, thread_root_post_id: i64) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM posts WHERE thread_root_post_id = $1 AND deleted_at IS NULL
             ORDER BY id DESC LIMIT 1",
        )
        .bind(thread_root_post_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn is_participant(&self, thread_root_post_id: i64, actor_id: i64) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                 SELECT 1 FROM posts p
                 WHERE p.thread_root_post_id = $1 AND p.deleted_at IS NULL
                   AND (
                       p.actor_id = $2
                       OR EXISTS (SELECT 1 FROM post_recipients pr WHERE pr.post_id = p.id AND pr.actor_id = $2)
                   )
             )",
        )
        .bind(thread_root_post_id)
        .bind(actor_id)
        .fetch_one(&self.pool)
        .await
    }

    async fn mark_read(&self, actor_id: i64, thread_root_post_id: i64, last_read_post_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO dm_read_states (actor_id, thread_root_post_id, last_read_post_id, updated_at)
             VALUES ($1, $2, $3, now())
             ON CONFLICT (actor_id, thread_root_post_id) DO UPDATE
             SET last_read_post_id = GREATEST(dm_read_states.last_read_post_id, EXCLUDED.last_read_post_id),
                 updated_at = now()",
        )
        .bind(actor_id)
        .bind(thread_root_post_id)
        .bind(last_read_post_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn unread_session_count(&self, actor_id: i64) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM (
                 SELECT DISTINCT p.thread_root_post_id
                 FROM posts p
                 WHERE p.thread_root_post_id IS NOT NULL AND p.deleted_at IS NULL
                   AND EXISTS (SELECT 1 FROM post_recipients pr WHERE pr.post_id = p.id AND pr.actor_id = $1)
                   AND p.id > COALESCE(
                       (SELECT last_read_post_id FROM dm_read_states WHERE actor_id = $1 AND thread_root_post_id = p.thread_root_post_id),
                       0
                   )
             ) sub",
        )
        .bind(actor_id)
        .fetch_one(&self.pool)
        .await
    }

    async fn recipient_ids(&self, post_id: i64) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar::<_, i64>("SELECT actor_id FROM post_recipients WHERE post_id = $1")
            .bind(post_id)
            .fetch_all(&self.pool)
            .await
    }

    async fn read_states(&self, actor_id: i64, thread_root_post_ids: &[i64]) -> Result<Vec<(i64, i64)>, sqlx::Error> {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT thread_root_post_id, last_read_post_id FROM dm_read_states
             WHERE actor_id = $1 AND thread_root_post_id = ANY($2)",
        )
        .bind(actor_id)
        .bind(thread_root_post_ids)
        .fetch_all(&self.pool)
        .await
    }

    async fn peer_summaries(&self, actor_ids: &[i64]) -> Result<Vec<DmPeerSummary>, sqlx::Error> {
        sqlx::query_as::<_, DmPeerSummary>(
            "SELECT a.id, a.username, a.domain, a.display_name, a.actor_type::text AS actor_type,
                    COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url
             FROM actors a
             LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
             LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
             WHERE a.id = ANY($1)",
        )
        .bind(actor_ids)
        .fetch_all(&self.pool)
        .await
    }
}

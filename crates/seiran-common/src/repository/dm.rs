//! ダイレクトメッセージ（`visibility='direct'`投稿のスレッド管理・既読状態）。
//!
//! 投稿本体（作成・タイムライン除外）は `post.rs` の `PostRepository` が担う。
//! ここでは「スレッド起点を同じくするdirect投稿の集合」をメッセージセッションとして
//! 扱うための一覧・履歴・既読状態のクエリのみを持つ。

use async_trait::async_trait;
use sqlx::{PgPool, Row};

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
    /// `viewer_actor_id`が著者または宛先であるメッセージのみを返す（`post_is_visible_to`で
    /// メッセージ単位に判定）。スレッド起点が同じでも、途中から加わった第三者宛の
    /// メッセージ等、viewerが関与しないメッセージは除外される（呼び出し元は別途
    /// `is_participant` でスレッドへのアクセス自体を確認すること）。
    async fn thread_messages(
        &self,
        thread_root_post_id: i64,
        viewer_actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// 指定スレッドの最新（id最大）ポストIDを取得する。既読カーソル更新に使う。
    async fn latest_post_id(&self, thread_root_post_id: i64) -> Result<Option<i64>, sqlx::Error>;

    /// 指定アクターが指定スレッドの参加者（投稿者 or 宛先のいずれか）かどうかを判定する。
    async fn is_participant(
        &self,
        thread_root_post_id: i64,
        actor_id: i64,
    ) -> Result<bool, sqlx::Error>;

    /// スレッドの最終既読ポストIDを記録する（`last_read_post_id`は単調増加のみ許可）。
    async fn mark_read(
        &self,
        actor_id: i64,
        thread_root_post_id: i64,
        last_read_post_id: i64,
    ) -> Result<(), sqlx::Error>;

    /// 未読のあるセッション数（バッジ表示用）。
    async fn unread_session_count(&self, actor_id: i64) -> Result<i64, sqlx::Error>;

    /// 複数スレッドの最終既読ポストIDを一括取得する（セッション一覧の未読フラグ算出用）。
    /// 戻り値は `(thread_root_post_id, last_read_post_id)` のタプル列（未読状態が無いスレッドは含まれない）。
    async fn read_states(
        &self,
        actor_id: i64,
        thread_root_post_ids: &[i64],
    ) -> Result<Vec<(i64, i64)>, sqlx::Error>;

    /// 投稿の宛先アクターID一覧を取得する（AP配送のto/cc組み立て用）。
    async fn recipient_ids(&self, post_id: i64) -> Result<Vec<i64>, sqlx::Error>;

    /// 複数投稿の宛先アクターIDを一括取得する（`(post_id, actor_id)`のペア列）。
    /// スレッド内の各メッセージごとの宛先表示（#DM宛先表示）用、N+1を避けるため一括で引く。
    async fn recipient_ids_for_posts(&self, post_ids: &[i64]) -> Result<Vec<(i64, i64)>, sqlx::Error>;

    /// セッション一覧の相手表示用に、複数アクターIDの要約情報を一括取得する。
    async fn peer_summaries(&self, actor_ids: &[i64]) -> Result<Vec<DmPeerSummary>, sqlx::Error>;

    /// 指定ポストがbsky宛DMメッセージ（スレッドが`bsky_convo_links`に登録済み）の場合のみ
    /// `Some`を返す。`chat.bsky.convo.addReaction`/`removeReaction`/`deleteMessageForSelf`の
    /// 呼び出しに必要な情報（convoId・対象メッセージのBsky側ID・閲覧者のDID/署名鍵）を
    /// まとめて取得する。
    async fn bsky_dm_context(
        &self,
        post_id: i64,
        viewer_actor_id: i64,
    ) -> Result<Option<BskyDmContext>, sqlx::Error>;

    /// bsky宛DMメッセージへの絵文字リアクションを追加する（`dm_bsky_reactions`）。
    /// 既に同じ`(post_id, actor_id, content)`があれば何もしない。戻り値は実際に新規追加
    /// されたか（`false`なら既存、Bsky側への`addReaction`送信もスキップしてよい）。
    async fn add_bsky_reaction(
        &self,
        id: i64,
        post_id: i64,
        actor_id: i64,
        content: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, sqlx::Error>;

    /// bsky宛DMメッセージへの絵文字リアクションを取り消す。戻り値は削除件数。
    async fn remove_bsky_reaction(
        &self,
        post_id: i64,
        actor_id: i64,
        content: &str,
    ) -> Result<u64, sqlx::Error>;

    /// 指定メッセージに付いているリアクションの総数（Bsky仕様上の1メッセージ最大5件制限
    /// のチェック用、全ユーザー合計）。
    async fn count_bsky_reactions(&self, post_id: i64) -> Result<i64, sqlx::Error>;

    /// Bsky受信ポーリング（`getMessages`）で取得した最新のリアクション一覧
    /// （`(actor_id, content)`のペア列）でDBの記録を完全同期する（既存のうち無くなった
    /// ものを削除、新規のものを追加）。戻り値は実際に変化があったか（呼び出し側が
    /// 無駄なWS配信をスキップするために使う）。
    async fn sync_bsky_reactions(
        &self,
        post_id: i64,
        reactions: &[(i64, String)],
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, sqlx::Error>;

    /// 指定メッセージを閲覧者自身の画面からだけ非表示にする
    /// （`chat.bsky.convo.deleteMessageForSelf`相当、`dm_hidden_messages`）。
    async fn hide_message(&self, actor_id: i64, post_id: i64) -> Result<(), sqlx::Error>;
}

/// `bsky_dm_context`の結果。`convo_id`がSomeであることは、対象ポストのスレッドが
/// Bsky宛DMであることを意味する（呼び出し元の「bsky宛DMかどうか」の判定はこれで行う）。
#[derive(Debug, Clone)]
pub struct BskyDmContext {
    pub convo_id: String,
    /// 対象メッセージ自身のBsky側ID。送信直後で`bsky_dm_send`のUPDATEがまだ完了して
    /// いない場合など、稀に`None`になりうる（呼び出し元はこの場合Bsky側操作を諦める）。
    pub bsky_message_id: Option<String>,
    /// 操作主体（閲覧者自身、常にローカルユーザー）のDID・署名鍵。
    pub viewer_did: String,
    pub viewer_pem: String,
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
             peers AS (
                 SELECT p.thread_root_post_id, p.actor_id AS peer_id
                 FROM posts p
                 WHERE p.thread_root_post_id IN (SELECT thread_root_post_id FROM my_threads) AND p.deleted_at IS NULL
                 UNION
                 SELECT p.thread_root_post_id, pr.actor_id AS peer_id
                 FROM posts p JOIN post_recipients pr ON pr.post_id = p.id
                 WHERE p.thread_root_post_id IN (SELECT thread_root_post_id FROM my_threads) AND p.deleted_at IS NULL
             ),
             -- 参加者（自分以外）が1人もいない、または全員がミュート/ブロック対象のスレッドは
             -- 一覧から除外する（ミュートは自分視点、ブロックは双方向＝seiranのブロック方針
             -- 「相互完全非表示」に合わせる。`handlers::target_resolve::check_not_blocked` 参照）。
             visible_threads AS (
                 SELECT DISTINCT mt.thread_root_post_id
                 FROM my_threads mt
                 WHERE NOT EXISTS (SELECT 1 FROM peers pe WHERE pe.thread_root_post_id = mt.thread_root_post_id AND pe.peer_id != $1)
                    OR EXISTS (
                        SELECT 1 FROM peers pe
                        WHERE pe.thread_root_post_id = mt.thread_root_post_id AND pe.peer_id != $1
                          AND NOT EXISTS (SELECT 1 FROM mutes m WHERE m.muter_actor_id = $1 AND m.muted_actor_id = pe.peer_id)
                          AND NOT EXISTS (
                              SELECT 1 FROM blocks b
                              WHERE (b.blocker_actor_id = $1 AND b.blocked_actor_id = pe.peer_id)
                                 OR (b.blocker_actor_id = pe.peer_id AND b.blocked_actor_id = $1)
                          )
                    )
             ),
             latest AS (
                 SELECT tr.thread_root_post_id, lp.id AS last_post_id, lp.body AS last_body, lp.created_at AS last_created_at
                 FROM visible_threads tr
                 JOIN LATERAL (
                     SELECT id, body, created_at FROM posts
                     WHERE thread_root_post_id = tr.thread_root_post_id AND deleted_at IS NULL
                     ORDER BY id DESC LIMIT 1
                 ) lp ON true
                 WHERE ($2::bigint IS NULL OR lp.id < $2)
                   AND ($3::bigint IS NULL OR lp.id > $3)
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
        viewer_actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets, p.content_html,
                    p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count,
                    p.reply_to_ap_uri, p.reply_to_ref_status::text AS reply_to_ref_status,
                    p.quote_of_ap_uri, p.quote_of_ref_status::text AS quote_of_ref_status,
                    p.repost_of_ap_uri, p.repost_of_ref_status::text AS repost_of_ref_status
             FROM posts p JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.thread_root_post_id = $1 AND p.deleted_at IS NULL
               AND ($4::bigint IS NULL OR p.id < $4)
               AND ($5::bigint IS NULL OR p.id > $5)
               AND post_is_visible_to($2, p.actor_id, p.visibility::text, p.id, false)
               AND NOT EXISTS (
                   SELECT 1 FROM dm_hidden_messages dh WHERE dh.actor_id = $2 AND dh.post_id = p.id
               )
             ORDER BY p.id ASC
             LIMIT $3",
        )
        .bind(thread_root_post_id)
        .bind(viewer_actor_id)
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

    async fn is_participant(
        &self,
        thread_root_post_id: i64,
        actor_id: i64,
    ) -> Result<bool, sqlx::Error> {
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

    async fn mark_read(
        &self,
        actor_id: i64,
        thread_root_post_id: i64,
        last_read_post_id: i64,
    ) -> Result<(), sqlx::Error> {
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
            "WITH my_threads AS (
                 SELECT DISTINCT p.thread_root_post_id
                 FROM posts p
                 WHERE p.thread_root_post_id IS NOT NULL AND p.deleted_at IS NULL
                   AND EXISTS (SELECT 1 FROM post_recipients pr WHERE pr.post_id = p.id AND pr.actor_id = $1)
             ),
             peers AS (
                 SELECT p.thread_root_post_id, p.actor_id AS peer_id
                 FROM posts p
                 WHERE p.thread_root_post_id IN (SELECT thread_root_post_id FROM my_threads) AND p.deleted_at IS NULL
                 UNION
                 SELECT p.thread_root_post_id, pr.actor_id AS peer_id
                 FROM posts p JOIN post_recipients pr ON pr.post_id = p.id
                 WHERE p.thread_root_post_id IN (SELECT thread_root_post_id FROM my_threads) AND p.deleted_at IS NULL
             ),
             -- sessions() と同じ「全参加者がミュート/ブロック対象のスレッドは除外」ロジック
             -- （新着バッジにミュート/ブロック済み相手からのDMを反映させないため）。
             visible_threads AS (
                 SELECT DISTINCT mt.thread_root_post_id
                 FROM my_threads mt
                 WHERE NOT EXISTS (SELECT 1 FROM peers pe WHERE pe.thread_root_post_id = mt.thread_root_post_id AND pe.peer_id != $1)
                    OR EXISTS (
                        SELECT 1 FROM peers pe
                        WHERE pe.thread_root_post_id = mt.thread_root_post_id AND pe.peer_id != $1
                          AND NOT EXISTS (SELECT 1 FROM mutes m WHERE m.muter_actor_id = $1 AND m.muted_actor_id = pe.peer_id)
                          AND NOT EXISTS (
                              SELECT 1 FROM blocks b
                              WHERE (b.blocker_actor_id = $1 AND b.blocked_actor_id = pe.peer_id)
                                 OR (b.blocker_actor_id = pe.peer_id AND b.blocked_actor_id = $1)
                          )
                    )
             )
             SELECT COUNT(*) FROM (
                 SELECT DISTINCT p.thread_root_post_id
                 FROM posts p
                 WHERE p.thread_root_post_id IN (SELECT thread_root_post_id FROM visible_threads)
                   AND p.deleted_at IS NULL
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

    async fn recipient_ids_for_posts(&self, post_ids: &[i64]) -> Result<Vec<(i64, i64)>, sqlx::Error> {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT post_id, actor_id FROM post_recipients WHERE post_id = ANY($1)",
        )
        .bind(post_ids)
        .fetch_all(&self.pool)
        .await
    }

    async fn read_states(
        &self,
        actor_id: i64,
        thread_root_post_ids: &[i64],
    ) -> Result<Vec<(i64, i64)>, sqlx::Error> {
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

    async fn bsky_dm_context(
        &self,
        post_id: i64,
        viewer_actor_id: i64,
    ) -> Result<Option<BskyDmContext>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT bcl.convo_id, p.bsky_message_id, va.at_did AS viewer_did, va.at_signing_key_pem AS viewer_pem
             FROM posts p
             JOIN bsky_convo_links bcl ON bcl.thread_root_post_id = COALESCE(p.thread_root_post_id, p.id)
             JOIN actors va ON va.id = $2
             WHERE p.id = $1",
        )
        .bind(post_id)
        .bind(viewer_actor_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.and_then(|r| {
            let viewer_did: Option<String> = r.try_get("viewer_did").ok().flatten();
            let viewer_pem: Option<String> = r.try_get("viewer_pem").ok().flatten();
            match (viewer_did, viewer_pem) {
                (Some(viewer_did), Some(viewer_pem)) => Some(BskyDmContext {
                    convo_id: r.try_get("convo_id").unwrap_or_default(),
                    bsky_message_id: r.try_get("bsky_message_id").unwrap_or(None),
                    viewer_did,
                    viewer_pem,
                }),
                // 閲覧者がAT Protocol未対応のローカルユーザー（DID未発行）。
                _ => None,
            }
        }))
    }

    async fn add_bsky_reaction(
        &self,
        id: i64,
        post_id: i64,
        actor_id: i64,
        content: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, sqlx::Error> {
        let inserted: Option<(i64,)> = sqlx::query_as(
            "INSERT INTO dm_bsky_reactions (id, post_id, actor_id, content, created_at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (post_id, actor_id, content) DO NOTHING
             RETURNING id",
        )
        .bind(id)
        .bind(post_id)
        .bind(actor_id)
        .bind(content)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        Ok(inserted.is_some())
    }

    async fn remove_bsky_reaction(
        &self,
        post_id: i64,
        actor_id: i64,
        content: &str,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "DELETE FROM dm_bsky_reactions WHERE post_id = $1 AND actor_id = $2 AND content = $3",
        )
        .bind(post_id)
        .bind(actor_id)
        .bind(content)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn count_bsky_reactions(&self, post_id: i64) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM dm_bsky_reactions WHERE post_id = $1")
            .bind(post_id)
            .fetch_one(&self.pool)
            .await
    }

    async fn sync_bsky_reactions(
        &self,
        post_id: i64,
        reactions: &[(i64, String)],
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let existing: Vec<(i64, String)> = sqlx::query_as(
            "SELECT actor_id, content FROM dm_bsky_reactions WHERE post_id = $1",
        )
        .bind(post_id)
        .fetch_all(&mut *tx)
        .await?;

        let existing_set: std::collections::HashSet<(i64, String)> =
            existing.into_iter().collect();
        let latest_set: std::collections::HashSet<(i64, String)> =
            reactions.iter().cloned().collect();
        let changed = existing_set != latest_set;

        for (actor_id, content) in latest_set.difference(&existing_set) {
            sqlx::query(
                "INSERT INTO dm_bsky_reactions (id, post_id, actor_id, content, created_at)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (post_id, actor_id, content) DO NOTHING",
            )
            .bind(crate::generate_snowflake_id(now))
            .bind(post_id)
            .bind(actor_id)
            .bind(content)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        for (actor_id, content) in existing_set.difference(&latest_set) {
            sqlx::query(
                "DELETE FROM dm_bsky_reactions WHERE post_id = $1 AND actor_id = $2 AND content = $3",
            )
            .bind(post_id)
            .bind(actor_id)
            .bind(content)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(changed)
    }

    async fn hide_message(&self, actor_id: i64, post_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO dm_hidden_messages (actor_id, post_id) VALUES ($1, $2)
             ON CONFLICT (actor_id, post_id) DO NOTHING",
        )
        .bind(actor_id)
        .bind(post_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

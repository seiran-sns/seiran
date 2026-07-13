use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// タイムライン表示用のポスト + アクター結合行。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TimelinePost {
    pub id: i64,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub actor_id: i64,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    // 7.2 拡張フィールド（古いクエリとの互換のため #[sqlx(default)] を付与）
    #[sqlx(default)]
    pub actor_type: String,
    #[sqlx(default)]
    pub repost_of_post_id: Option<i64>,
    #[sqlx(default)]
    pub quote_of_post_id: Option<i64>,
    #[sqlx(default)]
    pub reply_to_post_id: Option<i64>,
    #[sqlx(default)]
    pub parent_original_post_id: Option<i64>,
    /// 投稿者アバター URL（local は avatar_media_id 解決、remote は actors.avatar_url）。
    #[sqlx(default)]
    pub avatar_url: Option<String>,
}

/// プロフィール表示用のポスト要約。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PostSummary {
    pub id: i64,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

/// XRPC getRecord 用のレコード行。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PostRecord {
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub at_uri: String,
    pub at_cid: String,
}

/// リポスト・リプライ・引用の配送先を判定するために必要な、元ポストのメタ情報。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PostDeliveryMeta {
    pub ap_object_id: Option<String>,
    pub at_uri: Option<String>,
    pub at_cid: Option<String>,
    pub domain: String,
    pub display_name: Option<String>,
    pub username: String,
}

/// リポスト取り消し（Undo）に必要な情報。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RepostUndoInfo {
    pub repost_id: i64,
    pub repost_ap_id: Option<String>,
    pub orig_ap_id: Option<String>,
    pub atp_repost_rkey: Option<String>,
}

#[async_trait]
pub trait PostRepository: Send + Sync {
    /// 新規ポストを挿入する。
    async fn insert(
        &self,
        id: i64,
        actor_id: i64,
        body: &str,
        ap_object_id: &str,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// ホームタイムライン（自分 + フォロー中の accepted アクターの投稿）を取得する。
    async fn home_timeline(
        &self,
        actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// ローカルタイムライン（ローカルアクターの投稿）を取得する。
    async fn local_timeline(
        &self,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// 指定アクターの最近の投稿を取得する（プロフィール要約用の軽量版）。
    async fn recent_by_actor(
        &self,
        actor_id: i64,
        limit: i64,
    ) -> Result<Vec<PostSummary>, sqlx::Error>;

    /// 指定アクターの最近の投稿を、タイムラインと同じ結合行（アクター情報込み）で取得する。
    /// プロフィール画面でタイムラインと同一の NoteCard を描画するために使う（#43）。
    async fn timeline_by_actor(
        &self,
        actor_id: i64,
        limit: i64,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// DID + rkey で app.bsky.feed.post レコードを取得する。
    async fn find_record(&self, did: &str, rkey: &str) -> Result<Option<PostRecord>, sqlx::Error>;

    /// ID でポストとアクター情報を取得する。
    async fn find_by_id(&self, id: i64) -> Result<Option<TimelinePost>, sqlx::Error>;

    /// リモートノートを重複無視で挿入する（ON CONFLICT DO NOTHING）。
    async fn insert_remote(
        &self,
        id: i64,
        actor_id: i64,
        body: &str,
        ap_object_id: &str,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// 指定ノートIDより前（id < note_id）の投稿を降順で取得する。
    async fn context_before(
        &self,
        actor_id: i64,
        note_id: i64,
        limit: i64,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// 指定ノートIDより後（id > note_id）の投稿を昇順で取得する。
    async fn context_after(
        &self,
        actor_id: i64,
        note_id: i64,
        limit: i64,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// リポスト・リプライ・引用の配送先判定に使う、元ポストのメタ情報を取得する。
    async fn find_delivery_meta(&self, id: i64) -> Result<Option<PostDeliveryMeta>, sqlx::Error>;

    /// `seiran_post_uuid` / リプライ / 引用を含むローカル投稿を挿入する。
    #[allow(clippy::too_many_arguments)]
    async fn insert_full(
        &self,
        id: i64,
        actor_id: i64,
        body: &str,
        ap_object_id: &str,
        seiran_post_uuid: &str,
        reply_to_post_id: Option<i64>,
        quote_of_post_id: Option<i64>,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// リポストレコードを挿入する（`UNIQUE(actor_id, repost_of_post_id)` 制約違反はそのまま呼び出し元へ伝播する）。
    async fn insert_repost(
        &self,
        id: i64,
        actor_id: i64,
        ap_object_id: &str,
        repost_of_post_id: i64,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// 添付ファイルを投稿に紐付ける（ローカルアップロード済みの `media_file_id` を持つケース）。
    async fn attach_media(&self, post_id: i64, media_file_id: i64, position: i16) -> Result<(), sqlx::Error>;

    /// リモート添付 URL を投稿に紐付ける（`media_file_id` を持たない受信投稿用）。
    async fn attach_remote_media_url(&self, post_id: i64, url: &str, position: i16) -> Result<(), sqlx::Error>;

    /// 指定アクターが `note_id` に対して行ったリポストの取り消しに必要な情報を取得する。
    async fn find_repost_undo_info(
        &self,
        actor_id: i64,
        note_id: i64,
    ) -> Result<Option<RepostUndoInfo>, sqlx::Error>;

    /// 投稿を id で論理削除する。
    async fn soft_delete_by_id(&self, id: i64) -> Result<(), sqlx::Error>;

    /// 投稿を `ap_object_id` で論理削除する（Undo(Announce) 受信時）。返り値は削除行数。
    async fn soft_delete_by_ap_object_id(&self, ap_object_id: &str) -> Result<u64, sqlx::Error>;

    /// `seiran_post_uuid` から (id, ap_object_id) を取得する（seiran 間マージ判定用）。
    async fn find_by_seiran_uuid(&self, uuid: &str) -> Result<Option<(i64, Option<String>)>, sqlx::Error>;

    /// `ap_object_id` を更新する（seiran_uuid マージで AP 側が後着した場合）。
    async fn update_ap_object_id(&self, id: i64, ap_object_id: &str) -> Result<(), sqlx::Error>;

    /// `at_uri` から id を取得する（ブリッジ重複検知用）。
    async fn find_id_by_at_uri(&self, at_uri: &str) -> Result<Option<i64>, sqlx::Error>;

    /// `ap_object_id` または `at_uri` から id を取得する（Announce の元ポスト検索用）。
    async fn find_id_by_ap_or_at_uri(&self, uri: &str) -> Result<Option<i64>, sqlx::Error>;

    /// `ap_object_id` から (id, actor_id) を取得する（Like/EmojiReact の対象ポスト特定用）。
    async fn find_id_and_actor_by_ap_object_id(
        &self,
        ap_object_id: &str,
    ) -> Result<Option<(i64, i64)>, sqlx::Error>;

    /// `at_uri` から (id, actor_id) を取得する（ATP `app.bsky.feed.like` の対象ポスト特定用）。
    async fn find_id_and_actor_by_at_uri(
        &self,
        at_uri: &str,
    ) -> Result<Option<(i64, i64)>, sqlx::Error>;

    /// リモートから受信したノートを、重複排除メタ（seiran_uuid・ループバック/ブリッジ紐付け）付きで挿入する。
    /// `ap_object_id` が既存なら何もしない。
    #[allow(clippy::too_many_arguments)]
    async fn insert_remote_with_dedup(
        &self,
        id: i64,
        actor_id: i64,
        body: &str,
        ap_object_id: &str,
        seiran_uuid: Option<&str>,
        parent_original_post_id: Option<i64>,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;
}

pub struct PgPostRepository {
    pool: PgPool,
}

impl PgPostRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PostRepository for PgPostRepository {
    async fn insert(
        &self,
        id: i64,
        actor_id: i64,
        body: &str,
        ap_object_id: &str,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO posts (id, actor_id, body, ap_object_id, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(actor_id)
        .bind(body)
        .bind(ap_object_id)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn home_timeline(
        &self,
        actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url
             FROM posts p JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.deleted_at IS NULL
               AND ($2::bigint IS NULL OR p.id < $2)
               AND ($3::bigint IS NULL OR p.id > $3)
               AND (p.actor_id = $1 OR p.actor_id IN (
                     SELECT target_actor_id FROM follows
                     WHERE follower_actor_id = $1 AND status = 'accepted'))
             ORDER BY p.id DESC LIMIT $4",
        )
        .bind(actor_id)
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn local_timeline(
        &self,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url
             FROM posts p JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.is_local = true AND p.deleted_at IS NULL
               AND ($1::bigint IS NULL OR p.id < $1)
               AND ($2::bigint IS NULL OR p.id > $2)
             ORDER BY p.id DESC LIMIT $3",
        )
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn recent_by_actor(
        &self,
        actor_id: i64,
        limit: i64,
    ) -> Result<Vec<PostSummary>, sqlx::Error> {
        sqlx::query_as::<_, PostSummary>(
            "SELECT id, body, created_at FROM posts
             WHERE actor_id = $1 AND deleted_at IS NULL
             ORDER BY id DESC LIMIT $2",
        )
        .bind(actor_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn timeline_by_actor(
        &self,
        actor_id: i64,
        limit: i64,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, p.actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url
             FROM posts p
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.actor_id = $1 AND p.deleted_at IS NULL
             ORDER BY p.id DESC
             LIMIT $2",
        )
        .bind(actor_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_record(&self, did: &str, rkey: &str) -> Result<Option<PostRecord>, sqlx::Error> {
        sqlx::query_as::<_, PostRecord>(
            "SELECT p.body, p.created_at, p.at_uri, p.at_cid
             FROM posts p
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE a.at_did = $1 AND p.at_rkey = $2 AND p.deleted_at IS NULL
             LIMIT 1",
        )
        .bind(did)
        .bind(rkey)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_by_id(&self, id: i64) -> Result<Option<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url
             FROM posts p JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.id = $1 AND p.deleted_at IS NULL
             LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn insert_remote(
        &self,
        id: i64,
        actor_id: i64,
        body: &str,
        ap_object_id: &str,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO posts (id, actor_id, body, ap_object_id, created_at)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (ap_object_id) DO NOTHING",
        )
        .bind(id)
        .bind(actor_id)
        .bind(body)
        .bind(ap_object_id)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn context_before(
        &self,
        actor_id: i64,
        note_id: i64,
        limit: i64,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, p.actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url
             FROM posts p
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.actor_id = $1 AND p.id < $2 AND p.deleted_at IS NULL
             ORDER BY p.id DESC
             LIMIT $3",
        )
        .bind(actor_id)
        .bind(note_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn context_after(
        &self,
        actor_id: i64,
        note_id: i64,
        limit: i64,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, p.actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url
             FROM posts p
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.actor_id = $1 AND p.id > $2 AND p.deleted_at IS NULL
             ORDER BY p.id ASC
             LIMIT $3",
        )
        .bind(actor_id)
        .bind(note_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_delivery_meta(&self, id: i64) -> Result<Option<PostDeliveryMeta>, sqlx::Error> {
        sqlx::query_as::<_, PostDeliveryMeta>(
            "SELECT p.ap_object_id, p.at_uri, p.at_cid,
                    a.domain, a.display_name, a.username
             FROM posts p
             JOIN actors a ON a.id = p.actor_id
             WHERE p.id = $1 AND p.deleted_at IS NULL
             LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn insert_full(
        &self,
        id: i64,
        actor_id: i64,
        body: &str,
        ap_object_id: &str,
        seiran_post_uuid: &str,
        reply_to_post_id: Option<i64>,
        quote_of_post_id: Option<i64>,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO posts (id, actor_id, body, ap_object_id, seiran_post_uuid, reply_to_post_id, quote_of_post_id, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(actor_id)
        .bind(body)
        .bind(ap_object_id)
        .bind(seiran_post_uuid)
        .bind(reply_to_post_id)
        .bind(quote_of_post_id)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn insert_repost(
        &self,
        id: i64,
        actor_id: i64,
        ap_object_id: &str,
        repost_of_post_id: i64,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO posts (id, actor_id, body, ap_object_id, repost_of_post_id, created_at)
             VALUES ($1, $2, '', $3, $4, $5)",
        )
        .bind(id)
        .bind(actor_id)
        .bind(ap_object_id)
        .bind(repost_of_post_id)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn attach_media(&self, post_id: i64, media_file_id: i64, position: i16) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO post_attachments (post_id, media_file_id, position) VALUES ($1, $2, $3)",
        )
        .bind(post_id)
        .bind(media_file_id)
        .bind(position)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn attach_remote_media_url(&self, post_id: i64, url: &str, position: i16) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO post_attachments (post_id, media_file_id, remote_url, position)
             VALUES ($1, NULL, $2, $3)
             ON CONFLICT (post_id, position) DO NOTHING",
        )
        .bind(post_id)
        .bind(url)
        .bind(position)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn find_repost_undo_info(
        &self,
        actor_id: i64,
        note_id: i64,
    ) -> Result<Option<RepostUndoInfo>, sqlx::Error> {
        sqlx::query_as::<_, RepostUndoInfo>(
            "SELECT p.id AS repost_id, p.ap_object_id AS repost_ap_id,
                    p.atp_repost_rkey,
                    orig.ap_object_id AS orig_ap_id
             FROM posts p
             JOIN posts orig ON orig.id = p.repost_of_post_id
             WHERE p.actor_id = $1 AND p.repost_of_post_id = $2 AND p.deleted_at IS NULL
             LIMIT 1",
        )
        .bind(actor_id)
        .bind(note_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn soft_delete_by_id(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE posts SET deleted_at = NOW() WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn soft_delete_by_ap_object_id(&self, ap_object_id: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("UPDATE posts SET deleted_at = NOW() WHERE ap_object_id = $1")
            .bind(ap_object_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn find_by_seiran_uuid(&self, uuid: &str) -> Result<Option<(i64, Option<String>)>, sqlx::Error> {
        let row: Option<(i64, Option<String>)> = sqlx::query_as(
            "SELECT id, ap_object_id FROM posts WHERE seiran_post_uuid = $1 LIMIT 1",
        )
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn update_ap_object_id(&self, id: i64, ap_object_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE posts SET ap_object_id = $1 WHERE id = $2")
            .bind(ap_object_id)
            .bind(id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn find_id_by_at_uri(&self, at_uri: &str) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar::<_, i64>("SELECT id FROM posts WHERE at_uri = $1 LIMIT 1")
            .bind(at_uri)
            .fetch_optional(&self.pool)
            .await
    }

    async fn find_id_by_ap_or_at_uri(&self, uri: &str) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar::<_, i64>("SELECT id FROM posts WHERE ap_object_id = $1 OR at_uri = $1 LIMIT 1")
            .bind(uri)
            .fetch_optional(&self.pool)
            .await
    }

    async fn find_id_and_actor_by_ap_object_id(
        &self,
        ap_object_id: &str,
    ) -> Result<Option<(i64, i64)>, sqlx::Error> {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT id, actor_id FROM posts WHERE ap_object_id = $1 AND deleted_at IS NULL LIMIT 1",
        )
        .bind(ap_object_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_id_and_actor_by_at_uri(
        &self,
        at_uri: &str,
    ) -> Result<Option<(i64, i64)>, sqlx::Error> {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT id, actor_id FROM posts WHERE at_uri = $1 AND deleted_at IS NULL LIMIT 1",
        )
        .bind(at_uri)
        .fetch_optional(&self.pool)
        .await
    }

    async fn insert_remote_with_dedup(
        &self,
        id: i64,
        actor_id: i64,
        body: &str,
        ap_object_id: &str,
        seiran_uuid: Option<&str>,
        parent_original_post_id: Option<i64>,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO posts (id, actor_id, body, ap_object_id, seiran_post_uuid, parent_original_post_id, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (ap_object_id) DO NOTHING",
        )
        .bind(id)
        .bind(actor_id)
        .bind(body)
        .bind(ap_object_id)
        .bind(seiran_uuid)
        .bind(parent_original_post_id)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }
}

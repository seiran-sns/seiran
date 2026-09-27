//! ノート表示に付ける情報（添付・URLカード・リアクション集計・投票済みの選択肢・リポスト済み・
//! 返信/引用ゲート）を、複数の投稿分まとめて読む。frontend API と Misskey 互換 API の組み立て
//! （`build_note_responses`/`build_notes`）が共有する。

use sqlx::PgPool;

/// 投稿の添付1件。ローカルは `media_files` + `storage_providers` から URL を組み立て、
/// リモートは `remote_url` をそのまま使う。
#[derive(Debug, sqlx::FromRow)]
pub struct AttachmentRow {
    pub post_id: i64,
    pub url: Option<String>,
    pub mime_type: String,
    pub width: i32,
    pub height: i32,
    pub public_url: Option<String>,
    pub thumbnail_key: Option<String>,
    pub duration_ms: Option<i32>,
    pub remote_thumbnail_url: Option<String>,
    pub sha256: Option<String>,
    pub size: Option<i64>,
    pub media_created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_sensitive: bool,
    pub is_gif: bool,
    pub is_animated_image: bool,
}

/// 投稿ごとの添付を `position` 順に返す。
pub async fn attachments_for_posts(
    pool: &PgPool,
    post_ids: &[i64],
) -> Result<Vec<AttachmentRow>, sqlx::Error> {
    if post_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT pa.post_id,
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, pa.remote_url) AS url,
                COALESCE(mf.mime_type, pa.remote_mime_type, 'image/jpeg') AS mime_type,
                COALESCE(mf.width, 0) AS width,
                COALESCE(mf.height, 0) AS height,
                sp.public_url AS public_url,
                mf.thumbnail_key AS thumbnail_key,
                mf.duration_ms AS duration_ms,
                pa.remote_thumbnail_url AS remote_thumbnail_url,
                mf.sha256 AS sha256,
                mf.size AS size,
                mf.created_at AS media_created_at,
                pa.is_sensitive,
                pa.is_gif,
                COALESCE(mf.is_animated_image, FALSE) AS is_animated_image
         FROM post_attachments pa
         LEFT JOIN media_files mf ON mf.id = pa.media_file_id
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         WHERE pa.post_id = ANY($1)
         ORDER BY pa.post_id, pa.position",
    )
    .bind(post_ids)
    .fetch_all(pool)
    .await
}

#[derive(Debug, sqlx::FromRow)]
pub struct LinkCardRow {
    pub post_id: i64,
    pub url: String,
    pub title: String,
    pub description: String,
    pub thumbnail_url: Option<String>,
    pub embed_src: Option<String>,
    pub embed_type: Option<String>,
}

/// 投稿ごとの URL カードを `position` 順に返す。
pub async fn link_cards_for_posts(
    pool: &PgPool,
    post_ids: &[i64],
) -> Result<Vec<LinkCardRow>, sqlx::Error> {
    if post_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT post_id, url, title, description, thumbnail_url, embed_src, embed_type
         FROM post_link_cards
         WHERE post_id = ANY($1)
         ORDER BY post_id, position",
    )
    .bind(post_ids)
    .fetch_all(pool)
    .await
}

/// 投稿×絵文字ごとのリアクション件数。
#[derive(Debug, sqlx::FromRow)]
pub struct ReactionCountRow {
    pub post_id: i64,
    pub content: String,
    pub cnt: i64,
    pub emoji_url: Option<String>,
}

/// `reactions` を投稿×絵文字で集計する（件数の多い順）。`viewer_actor_id` がミュート・
/// ブロックしている相手のリアクションは数えない（Misskey 互換クライアントにも出さないため、
/// API の時点で除く）。
pub async fn reaction_counts_for_posts(
    pool: &PgPool,
    post_ids: &[i64],
    viewer_actor_id: Option<i64>,
) -> Result<Vec<ReactionCountRow>, sqlx::Error> {
    if post_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT post_id, content, COUNT(*) AS cnt, MAX(emoji_url) AS emoji_url
         FROM reactions
         WHERE post_id = ANY($1)
           AND ($2::bigint IS NULL OR NOT actor_is_hidden_for_viewer($2, actor_id))
         GROUP BY post_id, content
         ORDER BY post_id, cnt DESC",
    )
    .bind(post_ids)
    .bind(viewer_actor_id)
    .fetch_all(pool)
    .await
}

/// `actor_id` が付けているリアクション `(post_id, content)`。
pub async fn own_reactions_for_posts(
    pool: &PgPool,
    actor_id: i64,
    post_ids: &[i64],
) -> Result<Vec<(i64, String)>, sqlx::Error> {
    if post_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT post_id, content FROM reactions WHERE actor_id = $1 AND post_id = ANY($2)",
    )
    .bind(actor_id)
    .bind(post_ids)
    .fetch_all(pool)
    .await
}

/// `dm_bsky_reactions` を投稿×絵文字で集計する（件数の多い順）。`emoji_url` は常に NULL。
pub async fn dm_bsky_reaction_counts_for_posts(
    pool: &PgPool,
    post_ids: &[i64],
) -> Result<Vec<ReactionCountRow>, sqlx::Error> {
    if post_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT post_id, content, COUNT(*) AS cnt, NULL::text AS emoji_url
         FROM dm_bsky_reactions
         WHERE post_id = ANY($1)
         GROUP BY post_id, content
         ORDER BY post_id, cnt DESC",
    )
    .bind(post_ids)
    .fetch_all(pool)
    .await
}

/// `actor_id` が Bsky 宛 DM に付けているリアクション `(post_id, content)`。
pub async fn own_dm_bsky_reactions_for_posts(
    pool: &PgPool,
    actor_id: i64,
    post_ids: &[i64],
) -> Result<Vec<(i64, String)>, sqlx::Error> {
    if post_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT post_id, content FROM dm_bsky_reactions WHERE actor_id = $1 AND post_id = ANY($2)",
    )
    .bind(actor_id)
    .bind(post_ids)
    .fetch_all(pool)
    .await
}

/// `actor_id` が回答したアンケートの `(post_id, option_index)`。
pub async fn poll_votes_by_actor(
    pool: &PgPool,
    actor_id: i64,
    post_ids: &[i64],
) -> Result<Vec<(i64, i32)>, sqlx::Error> {
    if post_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT post_id, option_index FROM poll_votes
         WHERE actor_id = $1 AND post_id = ANY($2)
         ORDER BY post_id, option_index",
    )
    .bind(actor_id)
    .bind(post_ids)
    .fetch_all(pool)
    .await
}

/// `post_ids` のうち `actor_id` がリポスト済み（取り消していない）のもの。
pub async fn reposted_post_ids(
    pool: &PgPool,
    actor_id: i64,
    post_ids: &[i64],
) -> Result<Vec<i64>, sqlx::Error> {
    if post_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_scalar(
        "SELECT repost_of_post_id FROM posts
         WHERE actor_id = $1 AND repost_of_post_id = ANY($2) AND deleted_at IS NULL",
    )
    .bind(actor_id)
    .bind(post_ids)
    .fetch_all(pool)
    .await
}

/// メンション facet の DID をハンドルに置き換えるための、DID → アクターの対応。
#[derive(Debug, sqlx::FromRow)]
pub struct DidHandleRow {
    pub at_did: String,
    pub username: String,
    pub domain: String,
    pub actor_type: String,
}

pub async fn handles_for_dids(
    pool: &PgPool,
    dids: &[String],
) -> Result<Vec<DidHandleRow>, sqlx::Error> {
    if dids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT at_did, username, domain, actor_type::text AS actor_type
         FROM actors WHERE at_did = ANY($1)",
    )
    .bind(dids)
    .fetch_all(pool)
    .await
}

/// 返信/引用の制限（threadgate/postgate）を持つリモート Bsky 投稿。
#[derive(Debug, sqlx::FromRow)]
pub struct GateRow {
    pub id: i64,
    pub actor_id: i64,
    pub bsky_reply_allow: Option<serde_json::Value>,
    pub bsky_quote_disabled: bool,
    pub mention_facets: Option<serde_json::Value>,
}

/// `post_ids` のうち返信か引用に制限のあるものだけを返す。
pub async fn gated_posts(pool: &PgPool, post_ids: &[i64]) -> Result<Vec<GateRow>, sqlx::Error> {
    if post_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT id, actor_id, bsky_reply_allow, bsky_quote_disabled, mention_facets
         FROM posts
         WHERE id = ANY($1) AND (bsky_reply_allow IS NOT NULL OR bsky_quote_disabled)",
    )
    .bind(post_ids)
    .fetch_all(pool)
    .await
}

/// threadgate の `#listRule` 評価: `list_uri` がローカルユーザー所有のリストなら、`did` が
/// そのメンバーかを返す。ローカル所有でなければ `None`（リモートのキャッシュを見る）。
pub async fn local_list_has_member_did(
    pool: &PgPool,
    list_uri: &str,
    did: &str,
) -> Result<Option<bool>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM list_members lm JOIN actors a ON a.id = lm.actor_id
             WHERE lm.list_id = l.id AND a.at_did = $2
         )
         FROM lists l WHERE l.at_uri = $1",
    )
    .bind(list_uri)
    .bind(did)
    .fetch_optional(pool)
    .await
}

/// リモート Bsky リストのメンバー DID のキャッシュ（`bsky_remote_list_membership_cache`）。
#[derive(Debug, sqlx::FromRow)]
pub struct ListMembershipCacheRow {
    pub member_dids: serde_json::Value,
    pub checked_at: chrono::DateTime<chrono::Utc>,
}

pub async fn remote_list_membership_cache(
    pool: &PgPool,
    list_uri: &str,
) -> Result<Option<ListMembershipCacheRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT member_dids, checked_at FROM bsky_remote_list_membership_cache WHERE list_uri = $1",
    )
    .bind(list_uri)
    .fetch_optional(pool)
    .await
}

/// 保存する URL カード1件。`embed_src`/`embed_type` は許可リストで判定済みの値だけを渡す。
pub struct NewLinkCard<'a> {
    pub post_id: i64,
    pub position: i16,
    pub url: &'a str,
    pub title: &'a str,
    pub description: &'a str,
    pub thumbnail_url: Option<&'a str>,
    pub embed_src: Option<&'a str>,
    pub embed_type: Option<&'a str>,
}

pub async fn insert_link_card(pool: &PgPool, card: &NewLinkCard<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO post_link_cards (post_id, position, url, title, description, thumbnail_url, embed_src, embed_type)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(card.post_id)
    .bind(card.position)
    .bind(card.url)
    .bind(card.title)
    .bind(card.description)
    .bind(card.thumbnail_url)
    .bind(card.embed_src)
    .bind(card.embed_type)
    .execute(pool)
    .await
    .map(|_| ())
}

/// AP の `Document` として配るローカル添付（ストレージの公開 URL を持つもの）。
#[derive(Debug, sqlx::FromRow)]
pub struct LocalAttachmentRow {
    pub post_id: i64,
    pub url: String,
    pub mime_type: String,
    /// 動画・音声は持たないことがある。
    pub width: Option<i32>,
    pub height: Option<i32>,
}

/// 投稿ごとのローカル添付を `position` 順に返す。
pub async fn local_attachments_for_posts(
    pool: &PgPool,
    post_ids: &[i64],
) -> Result<Vec<LocalAttachmentRow>, sqlx::Error> {
    if post_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT pa.post_id,
                rtrim(sp.public_url, '/') || '/' || mf.storage_key AS url,
                mf.mime_type, mf.width, mf.height
         FROM post_attachments pa
         JOIN media_files mf ON mf.id = pa.media_file_id
         JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         WHERE pa.post_id = ANY($1)
         ORDER BY pa.post_id, pa.position",
    )
    .bind(post_ids)
    .fetch_all(pool)
    .await
}

/// 検索用に、投稿がブリッジポストかと、解決済みの元ポスト `(id, bridge_of_post_id, is_bridge)`。
pub async fn bridge_status_for_posts(
    pool: &PgPool,
    post_ids: &[i64],
) -> Result<Vec<(i64, Option<i64>, bool)>, sqlx::Error> {
    if post_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT id, bridge_of_post_id, (bridged_original_uri IS NOT NULL) AS is_bridge
         FROM posts WHERE id = ANY($1)",
    )
    .bind(post_ids)
    .fetch_all(pool)
    .await
}

pub async fn set_link_card_embed(
    pool: &PgPool,
    post_id: i64,
    position: i16,
    embed_src: &str,
    embed_type: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE post_link_cards SET embed_src = $1, embed_type = $2 WHERE post_id = $3 AND position = $4",
    )
    .bind(embed_src)
    .bind(embed_type)
    .bind(post_id)
    .bind(position)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn save_remote_list_membership_cache(
    pool: &PgPool,
    list_uri: &str,
    member_dids: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO bsky_remote_list_membership_cache (list_uri, member_dids, checked_at)
         VALUES ($1, $2, now())
         ON CONFLICT (list_uri) DO UPDATE SET member_dids = $2, checked_at = now()",
    )
    .bind(list_uri)
    .bind(member_dids)
    .execute(pool)
    .await
    .map(|_| ())
}

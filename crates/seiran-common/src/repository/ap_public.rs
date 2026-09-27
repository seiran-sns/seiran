//! 認証なしで公開する ActivityPub のエンドポイント（Actor 文書・outbox・featured・公開リスト・
//! WebFinger・nodeinfo）の読み取り。

use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// 未退会のローカルアクターの id。
pub async fn live_local_actor_id(
    pool: &PgPool,
    username: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM actors WHERE username = $1 AND actor_type = 'local' AND withdrawn_at IS NULL LIMIT 1",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
}

/// Actor 文書に載せる値。アバター・バナーは解決済みの URL。
#[derive(Debug, sqlx::FromRow)]
pub struct ActorDocumentRow {
    pub id: i64,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub avatar_mime_type: Option<String>,
    pub banner_url: Option<String>,
    pub banner_mime_type: Option<String>,
    pub profile_fields: Option<serde_json::Value>,
    pub emoji_map: Option<serde_json::Value>,
    pub birth_date: Option<chrono::NaiveDate>,
    pub birth_date_public: bool,
    pub is_locked: bool,
    pub at_did: Option<String>,
}

pub async fn actor_document(
    pool: &PgPool,
    username: &str,
) -> Result<Option<ActorDocumentRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.display_name, a.bio, \
                COALESCE(rtrim(avatar_sp.public_url, '/') || '/' || avatar_mf.storage_key, a.avatar_url) AS avatar_url, \
                avatar_mf.mime_type AS avatar_mime_type, \
                COALESCE(rtrim(banner_sp.public_url, '/') || '/' || banner_mf.storage_key, a.banner_url) AS banner_url, \
                banner_mf.mime_type AS banner_mime_type, \
                a.profile_fields, a.emoji_map, \
                a.birth_date, a.birth_date_public, a.is_locked, a.at_did \
         FROM actors a \
         LEFT JOIN media_files avatar_mf ON avatar_mf.id = a.avatar_media_id \
         LEFT JOIN storage_providers avatar_sp ON avatar_sp.id = avatar_mf.storage_provider_id \
         LEFT JOIN media_files banner_mf ON banner_mf.id = a.banner_media_id \
         LEFT JOIN storage_providers banner_sp ON banner_sp.id = banner_mf.storage_provider_id \
         WHERE a.username = $1 AND a.actor_type = 'local' AND a.withdrawn_at IS NULL LIMIT 1",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
}

/// 「別のアカウント」の対象 `(actor_type, username, ap_uri, at_did)`。
pub async fn also_known_as_targets(
    pool: &PgPool,
    owner_actor_id: i64,
) -> Result<Vec<(String, String, Option<String>, Option<String>)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.actor_type::text AS actor_type, a.username, a.ap_uri, a.at_did
         FROM actor_also_known_as aka
         JOIN actors a ON a.id = aka.target_actor_id
         WHERE aka.owner_actor_id = $1",
    )
    .bind(owner_actor_id)
    .fetch_all(pool)
    .await
}

/// 匿名に公開する投稿の条件（`followers_only`/`direct` を除く）。
const PUBLIC_POST_CONDITION: &str =
    "p.deleted_at IS NULL AND p.visibility NOT IN ('followers_only', 'direct')";

/// outbox の持ち主（未退会のローカルアクター）の id と公開投稿の総数。
pub async fn outbox_owner(
    pool: &PgPool,
    username: &str,
) -> Result<Option<(i64, i64)>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT a.id, COUNT(p.id) AS total
         FROM actors a
         LEFT JOIN posts p ON p.actor_id = a.id AND {PUBLIC_POST_CONDITION}
         WHERE a.username = $1 AND a.actor_type = 'local' AND a.withdrawn_at IS NULL
         GROUP BY a.id
         LIMIT 1"
    ))
    .bind(username)
    .fetch_optional(pool)
    .await
}

/// outbox の投稿1件。リポスト行は本文が空で単独では Create(Note) にできないので、
/// リポスト元の `ap_object_id`/`at_uri`/投稿者も持つ。
#[derive(Debug, sqlx::FromRow)]
pub struct OutboxRow {
    pub id: i64,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub repost_of_post_id: Option<i64>,
    pub ap_object_id: Option<String>,
    pub orig_ap_object_id: Option<String>,
    pub orig_at_uri: Option<String>,
    pub orig_username: Option<String>,
    pub orig_display_name: Option<String>,
    pub orig_actor_uri: Option<String>,
}

/// `max_id` より古い公開投稿を新しい順に最大 `limit` 件。
pub async fn outbox_page(
    pool: &PgPool,
    actor_id: i64,
    max_id: Option<i64>,
    limit: i64,
) -> Result<Vec<OutboxRow>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT p.id, p.body, p.created_at, p.repost_of_post_id, p.ap_object_id,
                orig.ap_object_id AS orig_ap_object_id, orig.at_uri AS orig_at_uri,
                oa.username AS orig_username, oa.display_name AS orig_display_name,
                oa.ap_uri AS orig_actor_uri
         FROM posts p
         LEFT JOIN posts orig ON orig.id = p.repost_of_post_id
         LEFT JOIN actors oa ON oa.id = orig.actor_id
         WHERE p.actor_id = $1 AND {PUBLIC_POST_CONDITION}
           AND ($2::BIGINT IS NULL OR p.id < $2)
         ORDER BY p.id DESC LIMIT $3"
    ))
    .bind(actor_id)
    .bind(max_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// ピン留めの公開投稿 `(id, body, created_at)`（ピン留めの新しい順）。
pub async fn featured_posts(
    pool: &PgPool,
    actor_id: i64,
) -> Result<Vec<(i64, String, DateTime<Utc>)>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT p.id, p.body, p.created_at
         FROM pinned_posts pp
         JOIN posts p ON p.id = pp.post_id
         WHERE pp.actor_id = $1 AND {PUBLIC_POST_CONDITION}
         ORDER BY pp.pinned_at DESC"
    ))
    .bind(actor_id)
    .fetch_all(pool)
    .await
}

pub async fn public_list_ids(pool: &PgPool, owner_actor_id: i64) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM lists WHERE owner_actor_id = $1 AND is_public = true ORDER BY created_at ASC",
    )
    .bind(owner_actor_id)
    .fetch_all(pool)
    .await
}

/// `username` のローカルアクターが所有する公開リストか。
pub async fn is_public_list_of(
    pool: &PgPool,
    list_id: i64,
    username: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM lists l
             JOIN actors a ON a.id = l.owner_actor_id
             WHERE l.id = $1 AND a.username = $2
               AND a.actor_type = 'local' AND l.is_public = true
         )",
    )
    .bind(list_id)
    .bind(username)
    .fetch_one(pool)
    .await
}

/// 公開リストの AP 系メンバー `(actor_type, username, ap_uri)`（追加の新しい順、Bsky を除く）。
pub async fn public_list_members(
    pool: &PgPool,
    list_id: i64,
) -> Result<Vec<(String, String, Option<String>)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.actor_type::text AS actor_type, a.username, a.ap_uri
         FROM list_members lm
         JOIN actors a ON a.id = lm.actor_id
         WHERE lm.list_id = $1 AND a.actor_type <> 'bsky'
         ORDER BY lm.added_at DESC",
    )
    .bind(list_id)
    .fetch_all(pool)
    .await
}

/// nodeinfo の `(ローカルユーザー数, ローカル投稿数)`。
pub async fn nodeinfo_counts(pool: &PgPool) -> Result<(i64, i64), sqlx::Error> {
    sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM actors WHERE actor_type = 'local' AND withdrawn_at IS NULL),
             (SELECT COUNT(*) FROM posts WHERE is_local = true AND deleted_at IS NULL)",
    )
    .fetch_one(pool)
    .await
}

/// `site_settings` のうち `keys` の値。
pub async fn site_settings_values(
    pool: &PgPool,
    keys: &[&str],
) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as("SELECT key, value FROM site_settings WHERE key = ANY($1)")
        .bind(keys)
        .fetch_all(pool)
        .await
}

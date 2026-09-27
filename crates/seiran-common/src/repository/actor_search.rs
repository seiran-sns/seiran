//! アクターの検索。用途別に2種類: リスト編集・DM 宛先向けの部分一致（表示名と全ハンドル表記）、
//! 投稿欄のメンション候補向けのハンドル前方一致。どちらも既知のアクターだけを対象にする。

use sqlx::PgPool;

#[derive(Debug, sqlx::FromRow)]
pub struct ActorSearchRow {
    pub id: i64,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub actor_type: String,
    pub avatar_url: Option<String>,
}

/// 表示名・Fedi/Bsky/ローカルの各ハンドルを改行でつないだ文字列に対する部分一致
/// （`idx_actors_search_bigm` を使う）。`contains_pattern` は LIKE 用にエスケープ済みの `%...%`。
pub async fn search_contains(
    pool: &PgPool,
    contains_pattern: &str,
    limit: i64,
) -> Result<Vec<ActorSearchRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.username, a.domain, a.display_name, a.actor_type::text AS actor_type,
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url
         FROM actors a
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         WHERE (a.actor_type != 'local' OR a.user_id IS NOT NULL)
           AND a.withdrawn_at IS NULL
           AND LOWER(
             COALESCE(a.display_name, '')
             || E'\\n@' || a.username
             || CASE WHEN a.domain <> '' THEN '@' || a.domain ELSE '' END
             || CASE WHEN a.actor_type = 'local'
                     THEN E'\\n@' || a.username || '.' || a.domain ELSE '' END
           ) LIKE LOWER($1) ESCAPE '\\'
         ORDER BY a.username
         LIMIT $2",
    )
    .bind(contains_pattern)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// ハンドルの前方一致（`idx_actors_handle_prefix`・`idx_actors_local_bsky_handle_prefix` を
/// 個別に走査して UNION する）。`pattern` は `...%`、`local_bsky_pattern` は `\n@...%`。
pub async fn suggest_by_handle_prefix(
    pool: &PgPool,
    pattern: &str,
    local_bsky_pattern: &str,
    limit: i64,
) -> Result<Vec<ActorSearchRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.username, a.domain, a.display_name, a.actor_type::text AS actor_type,
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url
         FROM actors a
         JOIN (
           (SELECT id FROM actors
            WHERE LOWER(username || CASE WHEN domain <> '' THEN '@' || domain ELSE '' END)
                    LIKE LOWER($1) ESCAPE '\\'
            LIMIT $3)
           UNION
           (SELECT id FROM actors
            WHERE LOWER(CASE WHEN actor_type = 'local'
                             THEN E'\\n@' || username || '.' || domain END)
                    LIKE LOWER($2) ESCAPE '\\'
            LIMIT $3)
         ) candidate ON candidate.id = a.id
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         WHERE (a.actor_type != 'local' OR a.user_id IS NOT NULL)
           AND a.withdrawn_at IS NULL
         ORDER BY a.username
         LIMIT $3",
    )
    .bind(pattern)
    .bind(local_bsky_pattern)
    .bind(limit)
    .fetch_all(pool)
    .await
}

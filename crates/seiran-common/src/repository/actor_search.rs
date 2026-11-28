//! アクターの検索。用途別に2種類: リスト編集・DM 宛先向けの部分一致（表示名と全ハンドル表記）、
//! 投稿欄のメンション候補向けのハンドル前方一致。どちらも既知のアクターだけを対象にする。

use sqlx::PgPool;

#[derive(Debug, sqlx::FromRow)]
pub struct ActorSearchRow {
    pub id: i64,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    /// 表示名中のカスタム絵文字（`:shortcode:` → 画像URL、`emoji_map::to_string_map` で変換）。
    pub emoji_map: Option<serde_json::Value>,
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
        "SELECT a.id, a.username, a.domain, a.display_name, a.emoji_map, a.actor_type::text AS actor_type,
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
        "SELECT a.id, a.username, a.domain, a.display_name, a.emoji_map, a.actor_type::text AS actor_type,
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

/// `ids` のアクターを、`ids` の順に（見つからない ID は除く）。
pub async fn find_rows_by_ids(
    pool: &PgPool,
    ids: &[i64],
) -> Result<Vec<ActorSearchRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.username, a.domain, a.display_name, a.emoji_map, a.actor_type::text AS actor_type,
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url
         FROM unnest($1::bigint[]) WITH ORDINALITY AS i(id, ord)
         JOIN actors a ON a.id = i.id
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         ORDER BY i.ord",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
}

/// ユーザー検索の対象（Misskey の `origin`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorOrigin {
    Any,
    Local,
    Remote,
}

impl ActorOrigin {
    fn as_sql(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }
}

/// ユーザー検索（Misskey の `users/search`・Mastodon の `accounts/search`）の条件。
#[derive(Debug, Clone, Copy)]
pub struct UserSearchFilter<'a> {
    /// LIKE 用にエスケープ済みの `%...%`（表示名・各ハンドル表記・自己紹介の部分一致）。
    pub contains_pattern: &'a str,
    /// LIKE 用にエスケープ済みの `...%`（ユーザー名の前方一致。並び順にだけ使う）。
    pub prefix_pattern: &'a str,
    pub origin: ActorOrigin,
    /// 閲覧者。ブロック・ミュートの相手を除く。`None`（未ログイン）ならローカルのうち
    /// ログアウト中の閲覧者に見せてよいアクターだけにする（リモートのデータはローカル
    /// ユーザーのためのキャッシュなので、未ログインには見せない）。
    pub viewer: Option<i64>,
    /// 閲覧者が承認済みでフォローしている相手だけにする（Mastodon の `following`）。
    pub following_only: bool,
    /// 結果から除く ID（呼び出し側が先頭に置く完全一致のアクター）。
    pub exclude_id: Option<i64>,
    pub limit: i64,
    pub offset: i64,
}

/// 検索結果に出してよいアクター（`$1` が閲覧者）。ブロック・ミュートは
/// `actor_is_hidden_for_viewer` と同じ条件だが、関数を行ごとに呼ぶと一致件数の多い短い
/// 語で秒単位に遅くなるため、`NOT EXISTS` で直接書いて anti join にさせる。
macro_rules! user_search_visibility {
    () => {
        "(a.actor_type != 'local' OR a.user_id IS NOT NULL)
           AND a.withdrawn_at IS NULL
           AND a.suspended_at IS NULL
           AND ($1::bigint IS NOT NULL
                OR (a.actor_type = 'local' AND NOT a.hide_from_unauthenticated))
           AND NOT EXISTS (SELECT 1 FROM blocks b
                           WHERE b.blocker_actor_id = $1 AND b.blocked_actor_id = a.id)
           AND NOT EXISTS (SELECT 1 FROM blocks b
                           WHERE b.blocked_actor_id = $1 AND b.blocker_actor_id = a.id)
           AND NOT EXISTS (SELECT 1 FROM mutes m
                           WHERE m.muter_actor_id = $1 AND m.muted_actor_id = a.id)"
    };
}

/// `$3` の `origin` での絞り込み。
macro_rules! user_search_origin {
    () => {
        "CASE $3::text WHEN 'local' THEN a.actor_type = 'local'
                       WHEN 'remote' THEN a.actor_type != 'local'
                       ELSE TRUE END"
    };
}

/// `search_user_ids` の照合・並び（`$2` 部分一致、`$4` 除く ID、`$5` 件数、`$6` 開始位置、
/// `$7` ユーザー名の前方一致）。ユーザー名が語で始まる人、表示名かハンドルに一致した人、
/// 自己紹介だけに一致した人の順に、それぞれユーザー名順（同名は ID 順）に並べる。
macro_rules! user_search_match_and_order {
    () => {
        "actor_search_text(a.display_name, a.username, a.domain, a.actor_type,
                           a.at_handle, a.bio) LIKE LOWER($2) ESCAPE '\\'
           AND a.id IS DISTINCT FROM $4::bigint
         ORDER BY LOWER(a.username) LIKE LOWER($7) ESCAPE '\\' DESC,
                  actor_handle_search_text(a.display_name, a.username, a.domain, a.actor_type,
                                           a.at_handle) LIKE LOWER($2) ESCAPE '\\' DESC,
                  LOWER(a.username), a.id
         LIMIT $5 OFFSET $6"
    };
}

/// ユーザー検索の ID 一覧。照合は表示名・各ハンドル・自己紹介の部分一致（SQL 関数
/// `actor_search_text`、`idx_actors_search_text_bigm` を使う）で、並びは
/// `user_search_match_and_order!` のとおり。`offset` で続きを取る。
pub async fn search_user_ids(
    pool: &PgPool,
    f: &UserSearchFilter<'_>,
) -> Result<Vec<i64>, sqlx::Error> {
    // フォロー中だけなら follows から引く（相手は高々フォロー数なので、全アクターから
    // 部分一致で拾ってからフォローを確かめるより速い。条件を1文にまとめると、短い語で
    // 一致件数の多い索引走査が選ばれて秒単位かかる）。
    let query = if f.following_only {
        sqlx::query_scalar(concat!(
            "SELECT a.id FROM follows fo JOIN actors a ON a.id = fo.target_actor_id
             WHERE fo.follower_actor_id = $1 AND fo.status = 'accepted' AND ",
            user_search_visibility!(),
            " AND ",
            user_search_origin!(),
            " AND ",
            user_search_match_and_order!()
        ))
    } else {
        sqlx::query_scalar(concat!(
            "SELECT a.id FROM actors a WHERE ",
            user_search_visibility!(),
            " AND ",
            user_search_origin!(),
            " AND ",
            user_search_match_and_order!()
        ))
    };
    query
        .bind(f.viewer)
        .bind(f.contains_pattern)
        .bind(f.origin.as_sql())
        .bind(f.exclude_id)
        .bind(f.limit)
        .bind(f.offset)
        .bind(f.prefix_pattern)
        .fetch_all(pool)
        .await
}

/// ホストの絞り込み（Misskey の `users/search-by-username-and-host` の `host`）。
#[derive(Debug, Clone, Copy)]
pub enum HostFilter<'a> {
    Any,
    Local,
    /// `domain` の前方一致（LIKE 用にエスケープ済みの `...%`）。
    Prefix(&'a str),
}

/// ユーザー名の前方一致とホストでの検索（メンションの補完用）。閲覧者がフォローしている
/// 相手を先に、それぞれユーザー名順に並べる（Misskey と同じ）。`username_prefix` は
/// LIKE 用にエスケープ済みの `...%`（`@` を含まない）、`None` ならユーザー名で絞らない。
/// ユーザー名の前方一致は `user@host` 表記の前方一致と同じなので、その形で書いて
/// `idx_actors_handle_prefix` を使わせる。
pub async fn search_user_ids_by_username_and_host(
    pool: &PgPool,
    viewer: Option<i64>,
    username_prefix: Option<&str>,
    host: HostFilter<'_>,
    limit: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    let (host_mode, host_pattern) = match host {
        HostFilter::Any => ("any", None),
        HostFilter::Local => ("local", None),
        HostFilter::Prefix(p) => ("prefix", Some(p)),
    };
    sqlx::query_scalar(concat!(
        "SELECT a.id FROM actors a
         WHERE ",
        user_search_visibility!(),
        "
           AND ($2::text IS NULL
                OR LOWER(a.username || CASE WHEN a.domain <> '' THEN '@' || a.domain ELSE '' END)
                   LIKE LOWER($2) ESCAPE '\\')
           AND CASE $3::text WHEN 'local' THEN a.actor_type = 'local'
                             WHEN 'prefix' THEN a.actor_type != 'local'
                                  AND LOWER(a.domain) LIKE LOWER($4) ESCAPE '\\'
                             ELSE TRUE END
         ORDER BY EXISTS (
                    SELECT 1 FROM follows f
                    WHERE f.follower_actor_id = $1 AND f.target_actor_id = a.id
                      AND f.status = 'accepted') DESC,
                  LOWER(a.username), a.id
         LIMIT $5"
    ))
    .bind(viewer)
    .bind(username_prefix)
    .bind(host_mode)
    .bind(host_pattern)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// `ids` のうち `search_user_ids` の条件（表示名・ハンドルの一致を除く）を満たすもの。
/// 完全一致で引き当てたアクターを先頭に置く前に、検索結果に出してよい相手かを確かめる。
pub async fn filter_searchable_user_ids(
    pool: &PgPool,
    f: &UserSearchFilter<'_>,
    ids: &[i64],
) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar(concat!(
        "SELECT a.id FROM actors a
         WHERE a.id = ANY($2) AND ",
        user_search_visibility!(),
        " AND ",
        user_search_origin!(),
        " AND (NOT $4::bool OR EXISTS (
                 SELECT 1 FROM follows f
                 WHERE f.follower_actor_id = $1 AND f.target_actor_id = a.id
                   AND f.status = 'accepted'))"
    ))
    .bind(f.viewer)
    .bind(ids)
    .bind(f.origin.as_sql())
    .bind(f.following_only)
    .fetch_all(pool)
    .await
}

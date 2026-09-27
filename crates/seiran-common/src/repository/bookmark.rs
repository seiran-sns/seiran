//! ブックマーク（Mastodon 互換 API）。投稿本体は `find_visible_posts_by_ids` で可視性判定つきで
//! 取り直すので、ここは (ブックマークID, 投稿ID) の組だけを扱う。

use sqlx::PgPool;

use super::Page;

/// ブックマークする。既にしていれば何もしない。
pub async fn insert(
    pool: &PgPool,
    id: i64,
    actor_id: i64,
    post_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO bookmarks (id, actor_id, post_id) VALUES ($1, $2, $3)
         ON CONFLICT (actor_id, post_id) DO NOTHING",
    )
    .bind(id)
    .bind(actor_id)
    .bind(post_id)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn delete(pool: &PgPool, actor_id: i64, post_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM bookmarks WHERE actor_id = $1 AND post_id = $2")
        .bind(actor_id)
        .bind(post_id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// `post_ids` のうち `actor_id` がブックマーク済みのもの。
pub async fn bookmarked_among(
    pool: &PgPool,
    actor_id: i64,
    post_ids: &[i64],
) -> Result<Vec<i64>, sqlx::Error> {
    if post_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_scalar("SELECT post_id FROM bookmarks WHERE actor_id = $1 AND post_id = ANY($2)")
        .bind(actor_id)
        .bind(post_ids)
        .fetch_all(pool)
        .await
}

/// ブックマークを新しい順に `(ブックマークID, 投稿ID)` で返す。カーソルはブックマークID。
pub async fn list(
    pool: &PgPool,
    actor_id: i64,
    page: Page,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, post_id FROM bookmarks
         WHERE actor_id = $1
           AND ($2::bigint IS NULL OR id < $2)
           AND ($3::bigint IS NULL OR id > $3)
         ORDER BY id DESC LIMIT $4",
    )
    .bind(actor_id)
    .bind(page.until_id)
    .bind(page.since_id)
    .bind(page.limit)
    .fetch_all(pool)
    .await
}

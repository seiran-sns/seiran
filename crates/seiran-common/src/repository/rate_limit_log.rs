//! ロール別レート制限の記録と集計（`search_log`・`user_contact_log`・投稿数）。
//!
//! 「窓内の件数を数えて上限未満なら記録する」処理は、トランザクション内でアクター単位の
//! アドバイザリロックを取って直列化する。数える文と記録する文の間に同じアクターの別の
//! リクエストが割り込むと、並列のリクエストがすべて上限判定をすり抜けるため。

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};

async fn lock_actor(
    conn: &mut PgConnection,
    namespace: &str,
    actor_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, $2))")
        .bind(namespace)
        .bind(actor_id)
        .execute(conn)
        .await
        .map(|_| ())
}

/// `since` 以降の投稿数（リポスト・削除済みを含む）。
pub async fn count_posts_since(
    pool: &PgPool,
    actor_id: i64,
    since: DateTime<Utc>,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM posts WHERE actor_id = $1 AND created_at >= $2")
        .bind(actor_id)
        .bind(since)
        .fetch_one(pool)
        .await
}

/// `since` 以降の検索が `max` 回未満なら1回記録して真を返す。
pub async fn record_search_if_under_limit(
    pool: &PgPool,
    actor_id: i64,
    since: DateTime<Utc>,
    max: i64,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_actor(&mut tx, "search_log", actor_id).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM search_log WHERE actor_id = $1 AND created_at >= $2",
    )
    .bind(actor_id)
    .bind(since)
    .fetch_one(&mut *tx)
    .await?;
    if count >= max {
        return Ok(false);
    }
    sqlx::query("INSERT INTO search_log (actor_id) VALUES ($1)")
        .bind(actor_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// `since` 以降に話しかけた相手に `targets` を足したユニーク数が `max_unique` 以下なら、
/// 新しい相手を記録して真を返す（既に窓内で話しかけた相手は数え直さない）。
pub async fn record_contacts_if_under_limit(
    pool: &PgPool,
    actor_id: i64,
    since: DateTime<Utc>,
    targets: &std::collections::HashSet<i64>,
    max_unique: usize,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_actor(&mut tx, "user_contact_log", actor_id).await?;
    let existing: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT target_actor_id FROM user_contact_log
         WHERE actor_id = $1 AND created_at >= $2",
    )
    .bind(actor_id)
    .bind(since)
    .fetch_all(&mut *tx)
    .await?;
    let existing: std::collections::HashSet<i64> = existing.into_iter().collect();
    let new_targets: Vec<i64> = targets.difference(&existing).copied().collect();
    if existing.len() + new_targets.len() > max_unique {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO user_contact_log (actor_id, target_actor_id)
         SELECT $1, unnest($2::bigint[])",
    )
    .bind(actor_id)
    .bind(&new_targets)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

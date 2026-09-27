//! `seiranPost` 相互一致マージ成立後の後始末（関連行の付け替えと、統合された側の物理削除）。
//!
//! 同期処理（`PostRepository::finalize_post_merge`）で統合された側（doomed）は論理削除
//! 済みのため、ここで付け替える参照はマージ成立時点で存在した分に限られる。

use sqlx::PgPool;

/// doomed から survivor へ付け替える `(テーブル, カラム)`。`post_attachments` /
/// `post_link_cards` は `(post_id, position)` が主キーで、survivor 側も同じ内容を独立に
/// 保存済みのため付け替えず、doomed 側の行は物理削除の CASCADE で消す。
const REPARENT_SIMPLE: &[(&str, &str)] = &[
    ("bsky_convo_links", "thread_root_post_id"),
    ("dm_read_states", "last_read_post_id"),
    ("dm_read_states", "thread_root_post_id"),
    ("notifications", "note_id"),
    ("pinned_posts", "post_id"),
    ("poll_votes", "post_id"),
    ("post_hashtags", "post_id"),
    ("post_recipients", "post_id"),
    ("posts", "parent_original_post_id"),
    ("posts", "thread_root_post_id"),
    ("reactions", "post_id"),
    ("reports", "subject_post_id"),
];

/// 付け替えと同時に survivor のカウンタを加算する `posts` の自己参照カラム。カウンタの
/// トリガーは INSERT と `deleted_at` 遷移でしか発火しないため、付け替え単独では増えない。
const REPARENT_COUNTED: &[(&str, &str)] = &[
    ("reply_to_post_id", "reply_count"),
    ("quote_of_post_id", "quote_count"),
    ("repost_of_post_id", "repost_count"),
];

pub enum MergeCleanup {
    /// doomed がまだ論理削除されていない（物理削除は取り消せないので何もしない）。
    NotSoftDeleted,
    /// 既に物理削除済み。
    AlreadyGone,
    /// 完了。付け替えに失敗した `(対象, エラー)` を含む（UNIQUE 違反等。該当行は
    /// doomed と共に消える）。
    Done(Vec<(String, sqlx::Error)>),
}

pub async fn cleanup_merged_post(
    pool: &PgPool,
    survivor_post_id: i64,
    doomed_post_id: i64,
) -> Result<MergeCleanup, sqlx::Error> {
    let deleted: Option<bool> =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM posts WHERE id = $1")
            .bind(doomed_post_id)
            .fetch_optional(pool)
            .await?;
    match deleted {
        Some(true) => {}
        Some(false) => return Ok(MergeCleanup::NotSoftDeleted),
        None => return Ok(MergeCleanup::AlreadyGone),
    }

    let mut skipped = Vec::new();
    for (table, column) in REPARENT_SIMPLE {
        let sql = format!("UPDATE {table} SET {column} = $1 WHERE {column} = $2");
        if let Err(e) = sqlx::query(&sql)
            .bind(survivor_post_id)
            .bind(doomed_post_id)
            .execute(pool)
            .await
        {
            skipped.push((format!("{table}.{column}"), e));
        }
    }

    for (column, count_column) in REPARENT_COUNTED {
        let sql = format!(
            "WITH moved AS (
                 UPDATE posts SET {column} = $1 WHERE {column} = $2
                 RETURNING id
             )
             UPDATE posts SET {count_column} = {count_column} + (SELECT count(*) FROM moved)
             WHERE id = $1 AND EXISTS (SELECT 1 FROM moved)"
        );
        if let Err(e) = sqlx::query(&sql)
            .bind(survivor_post_id)
            .bind(doomed_post_id)
            .execute(pool)
            .await
        {
            skipped.push((format!("posts.{column}"), e));
        }
    }

    sqlx::query("DELETE FROM posts WHERE id = $1")
        .bind(doomed_post_id)
        .execute(pool)
        .await?;
    Ok(MergeCleanup::Done(skipped))
}

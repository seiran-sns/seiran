//! `seiranPost`相互一致マージ（#237）成立後の非同期クリーンアップ。
//!
//! 同期処理（`PostRepository::finalize_post_merge`）が既に`ap_object_id`/`at_uri`の
//! 付け替え・`doomed_post_id`の論理削除（`deleted_at`）・`parent_original_post_id`設定を
//! 完了させているため、`doomed_post_id`は既存の「`deleted_at IS NULL`を前提とする」読み取り
//! 規約に乗って即座にタイムライン等から消えている。このジョブはその後、実際に重い
//! 関連テーブルのFK付け替え（見えている状態には影響しない、内部整合性のための後始末）と、
//! `doomed_post_id`の物理削除を行う。`deleted_at`済みのため新規参照が増えることはなく、
//! 付け替え対象はマージ成立時点で存在した分だけに限定される。
//!
//! `post_attachments`/`post_link_cards`は`(post_id, position)`複合PRIMARY KEYを持ち、
//! survivor側が既に自分自身の添付・URLカードを独立に保存済み（AP/ATP双方が同じ内容を
//! それぞれ受信・保存するため）でありpositionが衝突しうる。単純付け替えはPK違反を招くため、
//! この2テーブルはFK付け替えの対象外とし、doomed側の重複行は物理削除時にCASCADEで消す。

use crate::queue::worker::JobContext;

/// `doomed_post_id`から`survivor_post_id`へFKを付け替えるテーブル・カラムの一覧
/// （`post_attachments`/`post_link_cards`を除く、複合PRIMARY KEYを持たないもののみ）。
/// reply_to/quote_of/repost_of は`posts`自己参照のため別途カウンタ調整を伴う
/// （`REPARENT_COUNTED`参照）。
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

/// 付け替えと同時に`survivor_post_id`側のカウンタ（`reply_count`/`quote_count`/
/// `repost_count`）を手動調整する必要がある自己参照カラム。トリガー
/// （`trg_posts_relation_counts_insert`/`_delete`）はINSERT時・`deleted_at`遷移時にしか
/// 発火しないため、`UPDATE ... SET reply_to_post_id = ...`のような付け替え単独では
/// カウンタが増減しない（`docs/protocols.md` 5節参照）。
const REPARENT_COUNTED: &[(&str, &str)] = &[
    ("reply_to_post_id", "reply_count"),
    ("quote_of_post_id", "quote_count"),
    ("repost_of_post_id", "repost_count"),
];

pub async fn handle(
    survivor_post_id: i64,
    doomed_post_id: i64,
    ctx: std::sync::Arc<JobContext>,
) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        tracing::warn!(
            "[PostMergeCleanup] DB pool 未設定のためスキップ (doomed_post_id={})",
            doomed_post_id
        );
        return Ok(());
    };

    // 削除予定行が本当に論理削除済みか確認する（同期処理を経ずに誤って積まれた場合の
    // セーフガード。物理削除は取り消せないため）。
    let deleted: Option<bool> =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM posts WHERE id = $1")
            .bind(doomed_post_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("posts検索失敗: {}", e))?;
    match deleted {
        Some(true) => {}
        Some(false) => {
            return Err(format!(
                "doomed_post_id={} はまだ論理削除されていません（finalize_post_merge未実行の可能性）",
                doomed_post_id
            ));
        }
        None => {
            // 既に物理削除済み（再実行等）。冪等に成功扱いで終了する。
            return Ok(());
        }
    }

    for (table, column) in REPARENT_SIMPLE {
        let sql = format!(
            "UPDATE {table} SET {column} = $1 WHERE {column} = $2",
            table = table,
            column = column
        );
        if let Err(e) = sqlx::query(&sql)
            .bind(survivor_post_id)
            .bind(doomed_post_id)
            .execute(pool)
            .await
        {
            // UNIQUE制約違反等（同一ユーザーがAP/ATP両経路で個別にpin/投票済み等の
            // レアケース）はログのみで続行する。該当行はdoomed側に残ったまま物理削除で
            // 消えるが、ベストエフォートのクリーンアップのため致命的扱いにしない。
            tracing::warn!(
                "[PostMergeCleanup] {}.{} 付け替え失敗（続行）: {}",
                table,
                column,
                e
            );
        }
    }

    for (column, count_column) in REPARENT_COUNTED {
        let sql = format!(
            "WITH moved AS (
                 UPDATE posts SET {column} = $1 WHERE {column} = $2
                 RETURNING id
             )
             UPDATE posts SET {count_column} = {count_column} + (SELECT count(*) FROM moved)
             WHERE id = $1 AND EXISTS (SELECT 1 FROM moved)",
            column = column,
            count_column = count_column
        );
        if let Err(e) = sqlx::query(&sql)
            .bind(survivor_post_id)
            .bind(doomed_post_id)
            .execute(pool)
            .await
        {
            tracing::warn!(
                "[PostMergeCleanup] posts.{} 付け替え・カウンタ調整失敗（続行）: {}",
                column,
                e
            );
        }
    }

    // post_attachments/post_link_cards はsurvivor側が既に自分自身の分を独立に持つため
    // 付け替えず、doomed側の重複行は物理削除（CASCADE）に任せる。
    if let Err(e) = sqlx::query("DELETE FROM posts WHERE id = $1")
        .bind(doomed_post_id)
        .execute(pool)
        .await
    {
        return Err(format!(
            "doomed_post_id={} の物理削除失敗: {}",
            doomed_post_id, e
        ));
    }

    tracing::info!(
        "[PostMergeCleanup] 完了: survivor_post_id={} doomed_post_id={}",
        survivor_post_id,
        doomed_post_id
    );
    Ok(())
}

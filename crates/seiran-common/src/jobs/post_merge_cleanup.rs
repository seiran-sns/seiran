//! `seiranPost`相互一致マージ（#237）成立後の非同期クリーンアップ。
//! 統合された側の参照を付け替えて物理削除する（`repository::post_merge`）。

use crate::queue::worker::JobContext;
use crate::repository::post_merge::{cleanup_merged_post, MergeCleanup};

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

    match cleanup_merged_post(pool, survivor_post_id, doomed_post_id)
        .await
        .map_err(|e| format!("doomed_post_id={} の後始末失敗: {}", doomed_post_id, e))?
    {
        MergeCleanup::NotSoftDeleted => Err(format!(
            "doomed_post_id={} はまだ論理削除されていません（finalize_post_merge未実行の可能性）",
            doomed_post_id
        )),
        MergeCleanup::AlreadyGone => Ok(()),
        MergeCleanup::Done(skipped) => {
            for (target, e) in skipped {
                tracing::warn!("[PostMergeCleanup] {} 付け替え失敗（続行）: {}", target, e);
            }
            tracing::info!(
                "[PostMergeCleanup] 完了: survivor_post_id={} doomed_post_id={}",
                survivor_post_id,
                doomed_post_id
            );
            Ok(())
        }
    }
}

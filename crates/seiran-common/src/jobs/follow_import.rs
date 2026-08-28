//! フォローインポート（#設定画面から改行区切りのID一覧を貼り付けて一括フォロー）の
//! 自己再enqueue型ジョブ。
//!
//! `follow_import_items` に `pending` が残っていれば1件処理し、成功・失敗を問わず
//! 自分自身を再度キューへ積む。対象が尽きるか `follow_import_requests.status` が
//! `running` でなくなったら（完了/キャンセル）再enqueueせず終了する。
//!
//! レート制限（`check_follow_rate_limit`）に引っかかった場合は該当itemを`pending`の
//! まま [`RATE_LIMIT_POLL_SECS`] 秒後の `enqueue_retry` で自分自身を再投入する。
//! これは WorkerEngine の指数バックオフ（`attempt`）を経由しない独自の再試行であり、
//! `Err` を返す通常のジョブ失敗（真のDBエラー等）とは意図的に区別している
//! （`retry_config_for` の `FollowImportProcess` エントリは後者専用）。

use std::sync::Arc;
use std::time::Duration;

use crate::follow_exec::{execute_follow, FollowOutcome};
use crate::queue::worker::{priority, JobContext};
use crate::rate_limit::{check_follow_rate_limit, CheckFollowRateLimitError};
use crate::repository::{FollowImportRepository, PgFollowImportRepository, PgSiteSettingsRepository};
use crate::traits::Job;

/// レート制限超過時、再チェックまで待つ時間。ローリングウィンドウのため、この間隔で
/// ポーリングすれば「枠が空いてから最大この時間だけ遅延して再開」できる
/// （`docs/architecture.md` 参照、`RemoteFollowListSync` の重複防止クールダウンと同系の値）。
const RATE_LIMIT_POLL_SECS: u64 = 300;

pub async fn handle(request_id: i64, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        return Err("[FollowImportProcess] DB pool 未設定".to_string());
    };
    let Some(follow_exec) = ctx.follow_exec.as_ref() else {
        return Err("[FollowImportProcess] FollowExecConfig 未設定".to_string());
    };

    let repo = PgFollowImportRepository::new(pool.clone());

    let Some(request) = repo
        .get_request(request_id)
        .await
        .map_err(|e| format!("[FollowImportProcess] リクエスト取得失敗: {}", e))?
    else {
        tracing::warn!(
            "[FollowImportProcess] request_id={} が見つかりません（終了）",
            request_id
        );
        return Ok(());
    };

    if request.status != "running" {
        tracing::info!(
            "[FollowImportProcess] request_id={} は status={} のため終了",
            request_id,
            request.status
        );
        return Ok(());
    }

    let now = chrono::Utc::now();

    let Some((item_id, target)) = repo
        .claim_next_item(request_id)
        .await
        .map_err(|e| format!("[FollowImportProcess] 次アイテム取得失敗: {}", e))?
    else {
        repo.mark_completed(request_id, now)
            .await
            .map_err(|e| format!("[FollowImportProcess] 完了マーク失敗: {}", e))?;
        tracing::info!("[FollowImportProcess] request_id={} 完了", request_id);
        return Ok(());
    };

    // レート制限チェック（フォロー全体で合算カウント、通常のフォローと同じ制限をそのまま適用）。
    let site_settings = PgSiteSettingsRepository::new(pool.clone());
    match check_follow_rate_limit(pool, &site_settings, request.actor_id).await {
        Ok(()) => {}
        Err(CheckFollowRateLimitError::Exceeded { .. }) => {
            tracing::info!(
                "[FollowImportProcess] request_id={} レート制限超過のため{}秒後に再試行",
                request_id,
                RATE_LIMIT_POLL_SECS
            );
            ctx.queue
                .enqueue_retry(
                    Job::FollowImportProcess { request_id },
                    priority::LOW,
                    0,
                    Duration::from_secs(RATE_LIMIT_POLL_SECS),
                )
                .await
                .map_err(|e| format!("[FollowImportProcess] レート制限リトライ再投入失敗: {}", e))?;
            return Ok(());
        }
        Err(CheckFollowRateLimitError::Db(e)) => {
            return Err(format!("[FollowImportProcess] レート制限チェック失敗: {}", e));
        }
    }

    let requester = follow_exec
        .actors
        .find_by_id(request.actor_id)
        .await
        .map_err(|e| format!("[FollowImportProcess] 実行者アクター取得失敗: {}", e))?
        .ok_or_else(|| "[FollowImportProcess] 実行者アクターが見つかりません".to_string())?;

    let result = execute_follow(
        &target,
        request.actor_id,
        &requester.username,
        pool,
        &ctx.ap_client,
        &ctx.queue,
        follow_exec,
    )
    .await;

    let succeeded = match &result {
        Ok(FollowOutcome::Accepted { .. }) | Ok(FollowOutcome::Pending { .. }) => true,
        Err(e) => {
            tracing::warn!(
                "[FollowImportProcess] request_id={} item_id={} target={} 失敗: {}",
                request_id,
                item_id,
                target,
                e
            );
            false
        }
    };

    repo.mark_item_result(item_id, succeeded, now)
        .await
        .map_err(|e| format!("[FollowImportProcess] 結果記録失敗: {}", e))?;

    // 成功・失敗を問わず次の1件処理を即座に再投入する（未処理が残っていなければ、
    // 次回実行時の claim_next_item が None を返して completed になる）。
    ctx.queue
        .enqueue(Job::FollowImportProcess { request_id }, priority::LOW)
        .await
        .map_err(|e| format!("[FollowImportProcess] 次回enqueue失敗: {}", e))?;

    Ok(())
}

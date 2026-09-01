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
//!
//! **`request_id` 単位の排他ロック**: このジョブは起動時リカバリ（`seiran-api`
//! `spawn_startup_tasks`）により、プロセス再起動のたびに `running` 状態の全リクエストが
//! 無条件で再enqueueされる。もし直前のチェーンがまだ生きていれば、同一 `request_id` に
//! 対して複数のジョブが同時に走ることになる（split-role構成でRedisキューを使う場合、
//! 複数APIレプリカがそれぞれ再enqueueする可能性もある）。`claim_next_item` 自体は
//! アイテム単位でアトミックなため二重処理は起きないが、複数チェーンが並行すると
//! `check_follow_rate_limit` のTOCTOUで上限をわずかに超過しうる。これを避けるため、
//! `handle` の冒頭で `pg_try_advisory_lock(request_id)` を取得できたジョブだけが処理を
//! 行い、取れなかった場合は（既に別のジョブが処理中とみなし）何もせず終了する
//! （re-enqueueもしない。動いている方のジョブが自分でチェーンを継続するため）。
//! advisory lock はセッションスコープのため、`PgPool` から都度借りる接続ではなく
//! `pool.acquire()` で明示的に確保した1本の接続を lock/unlock の両方に使う。
//!
//! 次のジョブの enqueue は、必ず unlock が完了した**後**に行う。もし unlock 前に
//! enqueue すると、別ワーカーがそのジョブを即座に dequeue して `pg_try_advisory_lock`
//! を試みた際にまだロックが残っていて失敗し、re-enqueueもされずチェーンが途切れて
//! しまう（そのジョブは「既に別のジョブが処理中」とみなして黙って終了するため）。
//! そのため `process_locked` は実際の enqueue を行わず、次にすべきこと（[`NextAction`]）
//! を返すだけにし、`handle` が unlock 後に実行する。

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use crate::follow_exec::{execute_follow, FollowOutcome};
use crate::queue::worker::{priority, JobContext};
use crate::rate_limit::{check_follow_rate_limit, CheckFollowRateLimitError};
use crate::repository::{
    FollowImportItemOutcome, FollowImportRepository, PgFollowImportRepository,
    PgSiteSettingsRepository,
};
use crate::traits::Job;

/// レート制限超過時、再チェックまで待つ時間。ローリングウィンドウのため、この間隔で
/// ポーリングすれば「枠が空いてから最大この時間だけ遅延して再開」できる
/// （`docs/architecture.md` 参照、`RemoteFollowListSync` の重複防止クールダウンと同系の値）。
const RATE_LIMIT_POLL_SECS: u64 = 300;

/// `process_locked` が返す「unlock後にやるべきこと」。
enum NextAction {
    /// 次の1件処理を即座に再投入する。
    Continue,
    /// レート制限超過。指定秒後に再投入する。
    RetryAfter(Duration),
    /// 対象が尽きた/リクエストが running でなくなった等、再投入しない。
    Stop,
}

pub async fn handle(request_id: i64, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        return Err("[FollowImportProcess] DB pool 未設定".to_string());
    };

    let Some(lock_conn) = crate::advisory_lock::try_acquire(pool, request_id).await? else {
        tracing::info!(
            "[FollowImportProcess] request_id={} は既に別のジョブが処理中のためスキップ",
            request_id
        );
        return Ok(());
    };

    let result = process_locked(request_id, pool, &ctx).await;

    crate::advisory_lock::release(lock_conn, request_id).await;

    let next = match result {
        Ok(next) => next,
        Err(e) => return Err(e),
    };

    match next {
        NextAction::Continue => ctx
            .queue
            .enqueue(Job::FollowImportProcess { request_id }, priority::LOW)
            .await
            .map_err(|e| format!("[FollowImportProcess] 次回enqueue失敗: {}", e)),
        NextAction::RetryAfter(delay) => ctx
            .queue
            .enqueue_retry(
                Job::FollowImportProcess { request_id },
                priority::LOW,
                0,
                delay,
            )
            .await
            .map_err(|e| format!("[FollowImportProcess] レート制限リトライ再投入失敗: {}", e)),
        NextAction::Stop => Ok(()),
    }
}

async fn process_locked(
    request_id: i64,
    pool: &PgPool,
    ctx: &JobContext,
) -> Result<NextAction, String> {
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
        return Ok(NextAction::Stop);
    };

    if request.status != "running" {
        tracing::info!(
            "[FollowImportProcess] request_id={} は status={} のため終了",
            request_id,
            request.status
        );
        return Ok(NextAction::Stop);
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
        return Ok(NextAction::Stop);
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
            return Ok(NextAction::RetryAfter(Duration::from_secs(
                RATE_LIMIT_POLL_SECS,
            )));
        }
        Err(CheckFollowRateLimitError::Db(e)) => {
            return Err(format!(
                "[FollowImportProcess] レート制限チェック失敗: {}",
                e
            ));
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

    let outcome = match &result {
        Ok(
            FollowOutcome::Accepted {
                already_following: true,
                ..
            }
            | FollowOutcome::Pending {
                already_following: true,
                ..
            },
        ) => FollowImportItemOutcome::AlreadyFollowing,
        Ok(FollowOutcome::Accepted { .. }) | Ok(FollowOutcome::Pending { .. }) => {
            FollowImportItemOutcome::Succeeded
        }
        Err(e) => {
            tracing::warn!(
                "[FollowImportProcess] request_id={} item_id={} target={} 失敗: {}",
                request_id,
                item_id,
                target,
                e
            );
            FollowImportItemOutcome::Failed
        }
    };

    repo.mark_item_result(item_id, outcome, now)
        .await
        .map_err(|e| format!("[FollowImportProcess] 結果記録失敗: {}", e))?;

    // 成功・失敗を問わず次の1件処理を即座に再投入する（未処理が残っていなければ、
    // 次回実行時の claim_next_item が None を返して completed になる）。
    Ok(NextAction::Continue)
}

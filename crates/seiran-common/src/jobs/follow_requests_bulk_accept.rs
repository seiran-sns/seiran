//! フォロー承認制（鍵アカウント）をOFFに切り替えた際、その時点で存在した承認待ち
//! （`follows.status = 'pending'`）フォローリクエスト全件を一括承認する。
//!
//! `account::update_lock`（設定画面のトグルOFF）から積まれる。フォロワー数に比例して
//! 時間がかかりうる（ローカルフォロワーはATPコミットを、Fediフォロワーは AP Accept 送信を
//! 伴うため）ため、`account_withdraw_unfollow_all` と同様の advisory lock 付きジョブとして
//! 実装している。

use std::sync::Arc;

use crate::follow_approval::{approve_pending_follow, ApprovalConfig};
use crate::queue::worker::JobContext;

pub async fn handle(actor_id: i64, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        tracing::warn!(
            "[FollowRequestsBulkAccept] DB pool 未設定のためスキップ (actor_id={})",
            actor_id
        );
        return Ok(());
    };

    let Some(lock_conn) = crate::advisory_lock::try_acquire(pool, actor_id).await? else {
        tracing::info!(
            "[FollowRequestsBulkAccept] actor_id={} は既に別のジョブが処理中のためスキップ",
            actor_id
        );
        return Ok(());
    };

    let result = process_locked(actor_id, pool, &ctx).await;

    crate::advisory_lock::release(lock_conn, actor_id).await;

    result
}

async fn process_locked(actor_id: i64, pool: &sqlx::PgPool, ctx: &JobContext) -> Result<(), String> {
    let Some(follow_exec) = ctx.follow_exec.as_ref() else {
        tracing::warn!(
            "[FollowRequestsBulkAccept] follow_exec 設定未注入のためスキップ (actor_id={})",
            actor_id
        );
        return Ok(());
    };

    let target = follow_exec
        .actors
        .find_by_id(actor_id)
        .await
        .map_err(|e| format!("ターゲット取得失敗: {}", e))?
        .ok_or_else(|| format!("ターゲットアクター '{}' が見つかりません", actor_id))?;

    let pending = follow_exec
        .follows
        .find_pending_followers_raw(actor_id)
        .await
        .map_err(|e| format!("承認待ち一覧取得失敗: {}", e))?;

    let cfg = ApprovalConfig {
        db: pool,
        follows: &follow_exec.follows,
        notifications: &follow_exec.notifications,
        atp_service: &follow_exec.atp_service,
        ap_client: &ctx.ap_client,
        stream_hub: &follow_exec.stream_hub,
        local_domain: follow_exec.local_domain.as_str(),
        ap_private_key_pem: &follow_exec.ap_private_key_pem,
    };

    for (follower_actor_id, _) in pending {
        let follower = match follow_exec.actors.find_by_id(follower_actor_id).await {
            Ok(Some(a)) => a,
            Ok(None) => continue,
            Err(e) => {
                tracing::error!(
                    "[FollowRequestsBulkAccept] フォロワー取得失敗 (follower_actor_id={}): {}",
                    follower_actor_id,
                    e
                );
                continue;
            }
        };

        if let Err(e) = approve_pending_follow(&cfg, &follower, &target).await {
            tracing::error!(
                "[FollowRequestsBulkAccept] 承認失敗 (follower_actor_id={}): {}",
                follower_actor_id,
                e
            );
        }
    }

    tracing::info!(
        "[FollowRequestsBulkAccept] 承認待ちフォローリクエストの一括承認完了: actor_id={}",
        actor_id
    );
    Ok(())
}

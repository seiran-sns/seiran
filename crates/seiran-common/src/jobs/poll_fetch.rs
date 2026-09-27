//! リモートアンケート（AP Question）の生存監視フォールバック。
//!
//! `Update(Question)`を送ってこない実装（`jobs::inbound_activity_process::update::handle_update`
//! 参照）への保険として、締切前かつ長時間未フェッチのpollを表示読み込み時に能動的に再GETし直す
//! （`AppState::enqueue_poll_fetch`、`handlers::notes::queries::enqueue_stale_poll_fetches`が積む）。
//! 取得結果は`Update(Question)`受理時と同じ`pollUpdated` WebSocketイベントで反映する。

use crate::ap::ApError;
use crate::jobs::inbound_activity_process::normalize_ap_poll;
use crate::queue::worker::JobContext;
use crate::streaming::broadcast_poll_update;

pub async fn handle(post_id: i64, ctx: std::sync::Arc<JobContext>) -> Result<(), String> {
    let Some(inbox) = ctx.inbox.clone() else {
        tracing::warn!(
            "[PollFetch] InboxContext 未設定のためスキップ (post_id={})",
            post_id
        );
        return Ok(());
    };

    let Some(target) = crate::repository::poll::poll_fetch_target(&inbox.db_pool, post_id)
        .await
        .map_err(|e| format!("PollFetch: posts検索失敗 (post_id={}): {}", post_id, e))?
    else {
        return Ok(());
    };
    let post_author_id = target.actor_id;
    if target.poll_update_received {
        // enqueue後にUpdate(Question)が届いていた場合はそちらが最新なので何もしない。
        return Ok(());
    }
    let Some(ap_object_id) = target.ap_object_id else {
        return Ok(());
    };

    let signing_key =
        crate::system_actor::system_signing_key(&inbox.local_domain, &inbox.ap_private_key_pem);
    let fetched = match ctx
        .ap_client
        .fetch_object(&ap_object_id, (&signing_key.0, &signing_key.1))
        .await
    {
        Ok(v) => v,
        Err(ApError::Gone(detail)) => {
            tracing::info!(
                "[PollFetch] {} が消失（404/410）のため諦めます: {}",
                ap_object_id,
                detail
            );
            return Ok(());
        }
        Err(e) => {
            return Err(format!(
                "PollFetch: 再フェッチ失敗（リトライ対象） uri={}: {}",
                ap_object_id, e
            ));
        }
    };

    let Some(poll) = normalize_ap_poll(&fetched) else {
        // Question でなくなっていた（削除・別種への変更等）／oneOf・anyOfが読めない。
        // 以後も叩き直し続けないよう poll_fetched_at だけ進めて諦める。
        crate::repository::poll::save_fetched_poll(&inbox.db_pool, post_id, None)
            .await
            .map_err(|e| format!("PollFetch: poll_fetched_at更新失敗: {}", e))?;
        return Ok(());
    };

    crate::repository::poll::save_fetched_poll(&inbox.db_pool, post_id, Some(&poll))
        .await
        .map_err(|e| format!("PollFetch: poll更新失敗: {}", e))?;

    broadcast_poll_update(
        &inbox.stream_hub,
        inbox.follow_repo.as_ref(),
        post_id,
        post_author_id,
        &poll,
    )
    .await;

    tracing::info!("[PollFetch] post_id={} のpollを再取得しました", post_id);
    Ok(())
}

//! ② AP 配送キュー (`ap_delivery`)
//!
//! ローカルアクターの AP アクティビティ（Create/Announce/Undo/Update/Delete/リアクション）を
//! Fedi フォロワーの Inbox へ配送する。API ハンドラは `Job::ApDelivery` を enqueue するだけで、
//! 宛先解決・署名 POST・リトライ（WorkerEngine の指数バックオフ）はすべてこちらで行う。

use std::sync::Arc;

use crate::ap::{
    deliver_ap_announce, deliver_ap_reaction, deliver_ap_undo_reaction, deliver_delete_actor,
    deliver_post_to_ap_followers, deliver_undo_announce, deliver_update_actor,
};
use crate::queue::worker::JobContext;
use crate::traits::ApDeliveryKind;

pub async fn handle(actor_id: i64, kind: ApDeliveryKind, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        tracing::warn!("[ApDelivery] DB pool 未設定のためスキップ (actor_id={})", actor_id);
        return Ok(());
    };
    let Some(cfg) = ctx.delivery.as_ref() else {
        tracing::warn!("[ApDelivery] 配送設定（DeliveryConfig）未注入のためスキップ (actor_id={})", actor_id);
        return Ok(());
    };
    // 鍵未設定はリトライしても直らないため、明示ログを残して破棄する（空文字で署名を試みない）
    let Some(private_pem) = cfg.ap_private_key_pem.as_deref().filter(|s| !s.is_empty()) else {
        tracing::error!("[ApDelivery] AP 秘密鍵未設定のため配送を破棄 (actor_id={})", actor_id);
        return Ok(());
    };

    let ap_client = &ctx.ap_client;
    let domain = cfg.local_domain.as_str();

    match kind {
        ApDeliveryKind::PostToFollowers { post_id, body, quote_url, in_reply_to } => {
            deliver_post_to_ap_followers(
                ap_client, pool, post_id, actor_id, domain, private_pem,
                body.as_deref(), quote_url.as_deref(), in_reply_to.as_deref(),
            )
            .await
            .map_err(|e| e.to_string())
        }
        ApDeliveryKind::Announce { post_id, original_ap_object_id } => {
            deliver_ap_announce(
                ap_client, pool, post_id, actor_id, domain, private_pem, &original_ap_object_id,
            )
            .await
            .map_err(|e| e.to_string())
        }
        ApDeliveryKind::UndoAnnounce { announce_post_id, original_ap_object_id } => {
            deliver_undo_announce(
                ap_client, pool, announce_post_id, actor_id, domain, private_pem,
                &original_ap_object_id,
            )
            .await
            .map_err(|e| e.to_string())
        }
        ApDeliveryKind::Reaction { post_id, activity_id, content, undo_prev } => {
            // 切替時: 旧リアクションの Undo を先に配送する。失敗しても新リアクションの
            // 配送は続行する（Undo だけリトライで再送すると新リアクションが二重になるため）。
            if let Some(prev) = undo_prev {
                if let Err(e) = deliver_ap_undo_reaction(
                    ap_client, pool, post_id, actor_id, domain, private_pem,
                    &prev.activity_id, &prev.content,
                )
                .await
                {
                    tracing::error!("[ApDelivery] 旧リアクション Undo 配送失敗（続行）: {}", e);
                }
            }
            deliver_ap_reaction(
                ap_client, pool, post_id, actor_id, domain, private_pem, &activity_id, &content,
            )
            .await
            .map_err(|e| e.to_string())
        }
        ApDeliveryKind::UndoReaction { post_id, prev_activity_id, content } => {
            deliver_ap_undo_reaction(
                ap_client, pool, post_id, actor_id, domain, private_pem,
                &prev_activity_id, &content,
            )
            .await
            .map_err(|e| e.to_string())
        }
        ApDeliveryKind::UpdateActor => {
            let Some(public_pem) = cfg.ap_public_key_pem.as_deref().filter(|s| !s.is_empty()) else {
                tracing::error!("[ApDelivery] AP 公開鍵未設定のため Update(Actor) を破棄 (actor_id={})", actor_id);
                return Ok(());
            };
            deliver_update_actor(ap_client, pool, actor_id, domain, private_pem, public_pem)
                .await
                .map_err(|e| e.to_string())
        }
        ApDeliveryKind::DeleteActor => {
            deliver_delete_actor(ap_client, pool, actor_id, domain, private_pem)
                .await
                .map_err(|e| e.to_string())
        }
    }
}

//! 退会処理: 自分がフォローしていた相手（フォロイー）全員への一括アンフォロー。
//!
//! 従来の退会処理はフォロワー側（Delete(Actor)配送）とATP #accountイベントのみで、
//! 自分がフォローしていた相手（フォロイー）側へは何も通知しておらず、リモート側に
//! フォロー関係が残り続ける不整合があった（2026-07-16 マイケル指摘）。フォロー数に
//! 比例して時間がかかりうるため、Delete(Actor)配送（`ApDelivery`）・list-relayの
//! プロキシフォロー同期（`ProxyFollowSync`）と同様にジョブ化する。
//!
//! `follows` 行はターゲットごとに処理の最後で削除するため、リトライ時は既に
//! 処理済みのターゲットが自然にスキップされる（`find_atp_rkey`/AP送信対象の判定は
//! `follows` 行が残っている前提のため、削除済みなら該当ターゲットへの処理は
//! 事実上のno-opになる）。

use std::sync::Arc;
use serde_json::json;
use tokio::sync::broadcast;

use crate::atp::service::AtpCommitService;
use crate::jetstream_control::touch_jetstream_wanted_dids;
use crate::queue::worker::JobContext;
use crate::repository::{ActorRepository, FollowRepository, PgActorRepository, PgFollowRepository};

pub async fn handle(actor_id: i64, username: String, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        tracing::warn!("[AccountWithdrawUnfollowAll] DB pool 未設定のためスキップ (actor_id={})", actor_id);
        return Ok(());
    };
    let Some(cfg) = ctx.delivery.as_ref() else {
        tracing::warn!("[AccountWithdrawUnfollowAll] 配送設定未注入のためスキップ (actor_id={})", actor_id);
        return Ok(());
    };
    let ap_private_key_pem = cfg.ap_private_key_pem.clone().unwrap_or_default();

    let actors: Arc<dyn ActorRepository> = Arc::new(PgActorRepository::new(pool.clone()));
    let follows: Arc<dyn FollowRepository> = Arc::new(PgFollowRepository::new(pool.clone()));

    // ATPコミット用のサービス。event_txはこのジョブ専用の使い捨てチャンネルで良い
    // （subscribeReposのリアルタイム購読者には届かないが、atp_repo_eventsテーブルへの
    // 記録自体は行われるため、他のRelayが再購読すれば最終的に一貫する。退会時の
    // フォロー解除にリアルタイム性は必須ではないと判断）。
    let (event_tx, _rx) = broadcast::channel(16);
    let atp_service = AtpCommitService::new(pool.clone(), Arc::new(event_tx), Arc::clone(&ctx.ap_client.http));

    let target_ids = follows
        .find_accepted_target_ids(actor_id)
        .await
        .map_err(|e| format!("フォロー先一覧取得失敗: {}", e))?;

    let local_actor_uri = format!("https://{}/users/{}", cfg.local_domain, username);
    let actor_key_id = format!("{}#main-key", local_actor_uri);

    for target_id in target_ids {
        let target = match actors.find_by_id(target_id).await {
            Ok(Some(a)) => a,
            Ok(None) => continue,
            Err(e) => {
                tracing::error!("[AccountWithdrawUnfollowAll] ターゲット取得失敗 (target_id={}): {}", target_id, e);
                continue;
            }
        };

        let atp_rkey = match follows.find_atp_rkey(actor_id, target_id).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("[AccountWithdrawUnfollowAll] atp_rkey 取得失敗 (target_id={}): {}", target_id, e);
                continue;
            }
        };

        if let Some(rkey) = atp_rkey.as_deref() {
            if let Err(e) = atp_service.commit_delete_follow(actor_id, rkey, chrono::Utc::now()).await {
                tracing::error!("[AccountWithdrawUnfollowAll] ATP delete commit 失敗 (target_id={}): {}", target_id, e);
            } else {
                touch_jetstream_wanted_dids(pool).await;
            }
        }

        if target.actor_type != "local" && target.actor_type != "bsky" {
            if let (Some(ap_inbox_url), Some(ap_uri)) = (target.ap_inbox_url.as_deref(), target.ap_uri.as_deref()) {
                let follow_id = format!("https://{}/activities/follow/{}", cfg.local_domain, target.id);
                let undo_activity = json!({
                    "@context": "https://www.w3.org/ns/activitystreams",
                    "type": "Undo",
                    "id": format!("{}/undo", follow_id),
                    "actor": local_actor_uri,
                    "object": {
                        "type": "Follow",
                        "id": follow_id,
                        "actor": local_actor_uri,
                        "object": ap_uri,
                    }
                });
                if let Ok(body) = serde_json::to_string(&undo_activity) {
                    if let Err(e) = ctx
                        .ap_client
                        .sign_and_post(ap_inbox_url, &body, &actor_key_id, &ap_private_key_pem)
                        .await
                    {
                        tracing::error!("[AccountWithdrawUnfollowAll] AP Undo Follow 送信失敗 (target_id={}): {}", target_id, e);
                    }
                }
            }
        }

        if let Err(e) = follows.delete_by_actors(actor_id, target_id).await {
            tracing::error!("[AccountWithdrawUnfollowAll] follows DELETE 失敗 (target_id={}): {}", target_id, e);
        }
    }

    tracing::info!("[AccountWithdrawUnfollowAll] フォロー先への一括アンフォロー完了: actor_id={}", actor_id);
    Ok(())
}

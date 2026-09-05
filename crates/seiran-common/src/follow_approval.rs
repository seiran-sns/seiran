//! ロック（承認制フォロー）中のローカルアクター宛てに届いた、承認待ち（`follows.status
//! = 'pending'`）フォローリクエストの承認・拒否実処理。
//!
//! `seiran-api::handlers::follow_requests`（設定画面からの単発の承認/拒否）と
//! `jobs::follow_requests_bulk_accept`（承認制OFF切替時、既存の承認待ち全件を一括承認）の
//! 両方から呼ばれる共有ロジック。`follow_exec.rs` と同じ理由（`AppState`/`JobContext`
//! どちらからも呼べるようにするため）で、必要な依存だけを [`ApprovalConfig`] として
//! 明示的に受け取る形にしている。

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::json;
use sqlx::PgPool;

use crate::ap::ApClient;
use crate::atp::service::AtpCommitService;
use crate::generate_snowflake_id;
use crate::jetstream_control::touch_jetstream_wanted_dids;
use crate::repository::{Actor, FollowRepository, NotificationKind, NotificationRepository};
use crate::streaming::StreamHub;

pub struct ApprovalConfig<'a> {
    pub db: &'a PgPool,
    pub follows: &'a Arc<dyn FollowRepository>,
    pub notifications: &'a Arc<dyn NotificationRepository>,
    pub atp_service: &'a Arc<AtpCommitService>,
    pub ap_client: &'a Arc<ApClient>,
    pub stream_hub: &'a Arc<StreamHub>,
    pub local_domain: &'a str,
    pub ap_private_key_pem: &'a str,
}

/// 承認待ちフォローリクエストを承認する。`follower` がローカルアクターなら、承認まで
/// 遅延させていたATPフォローコミットをここで初めて実行する（ロック中はフォローが
/// 「本当に成立」していない状態を保つため、`follow_exec::follow_local` は承認まで
/// ATPコミット自体を行わない）。`follower` がFediリモートアクターならAP Acceptを送る。
pub async fn approve_pending_follow(
    cfg: &ApprovalConfig<'_>,
    follower: &Actor,
    target: &Actor,
) -> Result<(), String> {
    if follower.actor_type == "local" {
        let target_did = target
            .at_did
            .clone()
            .ok_or_else(|| "ターゲットに ATP DID がありません".to_string())?;
        let now = chrono::Utc::now();
        let rkey = cfg
            .atp_service
            .commit_follow(follower.id, &target_did, now)
            .await
            .map_err(|e| format!("ATP コミット失敗: {}", e))?;
        cfg.follows
            .accept_and_set_rkey(follower.id, target.id, Some(&rkey))
            .await
            .map_err(|e| format!("follows UPDATE 失敗: {}", e))?;
        touch_jetstream_wanted_dids(cfg.db).await;

        cfg.stream_hub.publish_event(
            HashSet::from([follower.id]),
            "followAccepted",
            json!({
                "actor": {
                    "username": target.username,
                    "domain": cfg.local_domain,
                    "displayName": target.display_name,
                },
            }),
        );
        let notif_id = generate_snowflake_id(chrono::Utc::now());
        if let Err(e) = cfg
            .notifications
            .insert(
                notif_id,
                follower.id,
                NotificationKind::FollowRequestAccepted,
                Some(target.id),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
        {
            tracing::error!("[follow_approval] notifications INSERT 失敗: {}", e);
        }
    } else {
        cfg.follows
            .accept_and_set_rkey(follower.id, target.id, None)
            .await
            .map_err(|e| format!("follows UPDATE 失敗: {}", e))?;

        if follower.actor_type != "bsky" {
            if let (Some(inbox), Some(follower_ap_uri)) =
                (follower.ap_inbox_url.as_deref(), follower.ap_uri.as_deref())
            {
                let target_uri = format!("https://{}/users/{}", cfg.local_domain, target.username);
                let actor_key_id = format!("{}#main-key", target_uri);
                let object = cfg
                    .follows
                    .find_pending_follow_activity(follower.id, target.id)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| {
                        json!({ "type": "Follow", "actor": follower_ap_uri, "object": target_uri })
                    });
                let accept_id = format!(
                    "https://{}/accepts/{}",
                    cfg.local_domain,
                    generate_snowflake_id(chrono::Utc::now())
                );
                let accept = json!({
                    "@context": "https://www.w3.org/ns/activitystreams",
                    "type": "Accept",
                    "id": accept_id,
                    "actor": target_uri,
                    "object": object,
                });
                if let Ok(body) = serde_json::to_string(&accept) {
                    if let Err(e) = cfg
                        .ap_client
                        .sign_and_post(inbox, &body, &actor_key_id, cfg.ap_private_key_pem)
                        .await
                    {
                        tracing::error!("[follow_approval] AP Accept 送信失敗: {}", e);
                    }
                }
            }
        }
    }
    Ok(())
}

/// 承認待ちフォローリクエストを拒否する。`follower` がFediリモートアクターならAP Rejectを
/// 送った上で`follows`行を削除する（ローカルフォロワーの場合はATPコミット自体を
/// 行っていないため、DB行の削除のみでよい）。
pub async fn reject_pending_follow(
    cfg: &ApprovalConfig<'_>,
    follower: &Actor,
    target: &Actor,
) -> Result<(), String> {
    if follower.actor_type != "local" && follower.actor_type != "bsky" {
        if let (Some(inbox), Some(follower_ap_uri)) =
            (follower.ap_inbox_url.as_deref(), follower.ap_uri.as_deref())
        {
            let target_uri = format!("https://{}/users/{}", cfg.local_domain, target.username);
            let actor_key_id = format!("{}#main-key", target_uri);
            let object = cfg
                .follows
                .find_pending_follow_activity(follower.id, target.id)
                .await
                .ok()
                .flatten()
                .unwrap_or_else(
                    || json!({ "type": "Follow", "actor": follower_ap_uri, "object": target_uri }),
                );
            let reject_id = format!(
                "https://{}/rejects/{}",
                cfg.local_domain,
                generate_snowflake_id(chrono::Utc::now())
            );
            let reject = json!({
                "@context": "https://www.w3.org/ns/activitystreams",
                "type": "Reject",
                "id": reject_id,
                "actor": target_uri,
                "object": object,
            });
            if let Ok(body) = serde_json::to_string(&reject) {
                if let Err(e) = cfg
                    .ap_client
                    .sign_and_post(inbox, &body, &actor_key_id, cfg.ap_private_key_pem)
                    .await
                {
                    tracing::error!("[follow_approval] AP Reject 送信失敗: {}", e);
                }
            }
        }
    }

    cfg.follows
        .delete_by_actors(follower.id, target.id)
        .await
        .map_err(|e| format!("follows DELETE 失敗: {}", e))?;
    Ok(())
}

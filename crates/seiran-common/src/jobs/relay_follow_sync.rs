//! Fediverseリレー参加ジョブ（#140）。
//!
//! relay-agent 仮想アクター（[`crate::system_actor`]）が、管理者が登録したリレーの
//! inbox URL へ Follow/Undo を送信する。リレー本体は `actors`/`follows` テーブルには
//! 登録しない（Mastodon本家のリレー実装と同様、`fediverse_relays` テーブル単独で完結する）。
//! Accept/Reject の受信は `inbound_activity_process` 側で `follow_activity_id` 一致により
//! 直接 `fediverse_relays.status` を更新する（このジョブの担当ではない）。

use serde_json::json;
use std::sync::Arc;

use crate::queue::worker::JobContext;
use crate::repository::{PgRelayRepository, RelayRepository};
use crate::system_actor::resolve_relay_agent_actor_id;

pub async fn handle(relay_id: i64, want_follow: bool, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        tracing::warn!(
            "[RelayFollowSync] DB pool 未設定のためスキップ (relay_id={})",
            relay_id
        );
        return Ok(());
    };
    let Some(cfg) = ctx.delivery.as_ref() else {
        tracing::warn!(
            "[RelayFollowSync] 配送設定未注入のためスキップ (relay_id={})",
            relay_id
        );
        return Ok(());
    };
    let Some(private_pem) = cfg.ap_private_key_pem.as_deref().filter(|s| !s.is_empty()) else {
        tracing::error!(
            "[RelayFollowSync] AP 秘密鍵未設定のため破棄 (relay_id={})",
            relay_id
        );
        return Ok(());
    };

    // relay-agent actor_id 自体は使わないが、起動時ブートストラップ済みであることの確認に使う。
    resolve_relay_agent_actor_id(pool)
        .await
        .map_err(|e| format!("relay-agent actor_id 解決失敗: {}", e))?
        .ok_or_else(|| "relay-agent アクターが未初期化です".to_string())?;

    let relays: Arc<dyn RelayRepository> = Arc::new(PgRelayRepository::new(pool.clone()));

    let Some(relay) = relays
        .find_by_id(relay_id)
        .await
        .map_err(|e| format!("リレー取得失敗: {}", e))?
    else {
        // want_follow=false の場合、既に他経路で削除済みなら何もしなくてよい。
        tracing::warn!("[RelayFollowSync] リレー(id={})が見つかりません", relay_id);
        return Ok(());
    };

    let domain = cfg.local_domain.as_str();
    let relay_agent_uri = format!("https://{}/users/relay-agent", domain);
    let actor_key_id = format!("{}#main-key", relay_agent_uri);

    if want_follow {
        let follow_activity = json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "type": "Follow",
            "id": relay.follow_activity_id,
            "actor": relay_agent_uri,
            "object": relay.inbox_url,
        });
        let body = serde_json::to_string(&follow_activity).map_err(|e| e.to_string())?;
        ctx.ap_client
            .sign_and_post(&relay.inbox_url, &body, &actor_key_id, private_pem)
            .await
            .map_err(|e| format!("Follow送信失敗: {}", e))?;

        tracing::info!(
            "[RelayFollowSync] relay-agent → {} Follow送信完了 (pending)",
            relay.inbox_url
        );
    } else {
        let undo_activity = json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "type": "Undo",
            "id": format!("{}/undo", relay.follow_activity_id),
            "actor": relay_agent_uri,
            "object": {
                "type": "Follow",
                "id": relay.follow_activity_id,
                "actor": relay_agent_uri,
                "object": relay.inbox_url,
            }
        });
        let body = serde_json::to_string(&undo_activity).map_err(|e| e.to_string())?;
        ctx.ap_client
            .sign_and_post(&relay.inbox_url, &body, &actor_key_id, private_pem)
            .await
            .map_err(|e| format!("Undo Follow送信失敗: {}", e))?;

        relays
            .delete(relay_id)
            .await
            .map_err(|e| format!("fediverse_relays DELETE失敗: {}", e))?;

        tracing::info!(
            "[RelayFollowSync] relay-agent → {} 離脱完了",
            relay.inbox_url
        );
    }

    Ok(())
}

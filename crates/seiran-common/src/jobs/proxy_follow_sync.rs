//! プロキシフォロー同期ジョブ（リスト機能 #63）。
//!
//! 誰にもフォローされていないリモートFediユーザーの投稿を受信するため、
//! list-relay 仮想アクター（[`crate::system_actor`]）が代理でフォロー/アンフォローを行う。
//! 呼び出し元（`seiran-api::handlers::lists`）は「参照カウントが 0→1 になった／1→0 に
//! なった」タイミングでのみこのジョブを enqueue する（参照カウント自体の判定は
//! `ListRepository::actor_referenced_by_any_list` を使い呼び出し側で行い、このジョブは
//! Follow/Undo の実送信と `follows` テーブルの更新だけを担当する）。

use std::sync::Arc;
use serde_json::json;

use crate::queue::worker::JobContext;
use crate::repository::{ActorRepository, FollowRepository, PgActorRepository, PgFollowRepository};
use crate::system_actor::resolve_system_proxy_actor_id;

pub async fn handle(target_actor_id: i64, want_follow: bool, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        tracing::warn!("[ProxyFollowSync] DB pool 未設定のためスキップ (target={})", target_actor_id);
        return Ok(());
    };
    let Some(cfg) = ctx.delivery.as_ref() else {
        tracing::warn!("[ProxyFollowSync] 配送設定未注入のためスキップ (target={})", target_actor_id);
        return Ok(());
    };
    let Some(private_pem) = cfg.ap_private_key_pem.as_deref().filter(|s| !s.is_empty()) else {
        tracing::error!("[ProxyFollowSync] AP 秘密鍵未設定のため破棄 (target={})", target_actor_id);
        return Ok(());
    };

    let proxy_actor_id = resolve_system_proxy_actor_id(pool)
        .await
        .map_err(|e| format!("list-relay actor_id 解決失敗: {}", e))?
        .ok_or_else(|| "list-relay プロキシアクターが未初期化です".to_string())?;

    let actors: Arc<dyn ActorRepository> = Arc::new(PgActorRepository::new(pool.clone()));
    let follows: Arc<dyn FollowRepository> = Arc::new(PgFollowRepository::new(pool.clone()));

    let target = actors
        .find_by_id(target_actor_id)
        .await
        .map_err(|e| format!("ターゲットアクター取得失敗: {}", e))?
        .ok_or_else(|| format!("ターゲットアクター(id={})が見つかりません", target_actor_id))?;

    if target.actor_type != "fedi" {
        // Fedi以外（bsky/local）はプロキシフォロー対象外。呼び出し側の判定ミスでも
        // ここで弾いておけば follows テーブルを汚さない。
        return Ok(());
    }

    let (Some(target_uri), Some(target_inbox)) =
        (target.ap_uri.as_deref(), target.ap_inbox_url.as_deref())
    else {
        return Err(format!("ターゲットアクター(id={})にap_uri/ap_inbox_urlがありません", target_actor_id));
    };

    let existing_status = follows
        .find_status(proxy_actor_id, target_actor_id)
        .await
        .map_err(|e| format!("フォロー状態取得失敗: {}", e))?;

    let domain = cfg.local_domain.as_str();
    let proxy_actor_uri = format!("https://{}/users/list-relay", domain);
    let actor_key_id = format!("{}#main-key", proxy_actor_uri);
    let follow_id = format!("https://{}/activities/follow/list-relay-{}", domain, target_actor_id);

    if want_follow {
        if existing_status.is_some() {
            // 既にフォロー済み（他のリストが先に参照していた等）。何もしない。
            return Ok(());
        }

        let follow_activity = json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "type": "Follow",
            "id": follow_id,
            "actor": proxy_actor_uri,
            "object": target_uri,
        });
        let body = serde_json::to_string(&follow_activity).map_err(|e| e.to_string())?;
        ctx.ap_client
            .sign_and_post(target_inbox, &body, &actor_key_id, private_pem)
            .await
            .map_err(|e| format!("Follow送信失敗: {}", e))?;

        follows
            .upsert_pending(proxy_actor_id, target_actor_id)
            .await
            .map_err(|e| format!("follows INSERT失敗: {}", e))?;

        tracing::info!(
            "[ProxyFollowSync] list-relay → {} Follow送信完了 (pending)",
            target_uri
        );
    } else {
        if existing_status.is_none() {
            // 元々フォローしていない（何らかの理由で既に解除済み）。何もしない。
            return Ok(());
        }

        let undo_activity = json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "type": "Undo",
            "id": format!("{}/undo", follow_id),
            "actor": proxy_actor_uri,
            "object": {
                "type": "Follow",
                "id": follow_id,
                "actor": proxy_actor_uri,
                "object": target_uri,
            }
        });
        let body = serde_json::to_string(&undo_activity).map_err(|e| e.to_string())?;
        ctx.ap_client
            .sign_and_post(target_inbox, &body, &actor_key_id, private_pem)
            .await
            .map_err(|e| format!("Undo Follow送信失敗: {}", e))?;

        follows
            .delete_by_actors(proxy_actor_id, target_actor_id)
            .await
            .map_err(|e| format!("follows DELETE失敗: {}", e))?;

        tracing::info!(
            "[ProxyFollowSync] list-relay → {} アンフォロー完了",
            target_uri
        );
    }

    Ok(())
}

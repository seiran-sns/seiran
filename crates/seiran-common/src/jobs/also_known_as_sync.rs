//! プロフィールの「別のアカウント」（alsoKnownAs）表示のうち、リモートFediアクター
//! プロフィール表示時の同期ジョブ。ローカルユーザーが自分で登録する版
//! （`jobs::also_known_as_verify`）とは異なり、こちらはリモートアクター自身のAP actor
//! 文書が公開している`alsoKnownAs`をそのまま`actor_also_known_as`へ取り込む
//! （本人の自己申告をそのまま表示し、相互検証は`AlsoKnownAsVerify`へ委譲する）。
//!
//! `handlers::users::user_profile`がリモートFediアクターのプロフィール表示のたびに積む
//! （`AlsoKnownAsVerify`と同じ「表示時再検証」パターン、`docs/architecture.md`参照）。

use std::sync::Arc;

use crate::generate_snowflake_id;
use crate::queue::worker::{priority, JobContext};
use crate::repository::{
    ActorRepository, AlsoKnownAsRepository, PgActorRepository, PgAlsoKnownAsRepository,
};
use crate::traits::Job;

pub async fn handle(owner_actor_id: i64, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!("[RemoteAlsoKnownAsSync] DB pool 未設定のためスキップ");
        return Ok(());
    };
    let actors = PgActorRepository::new(pool.clone());
    let also_known_as = PgAlsoKnownAsRepository::new(pool.clone());

    let Some(owner) = actors
        .find_by_id(owner_actor_id)
        .await
        .map_err(|e| format!("owner取得失敗: {}", e))?
    else {
        return Ok(());
    };
    if owner.actor_type != "fedi" {
        // ローカル/Bskyアクターはここへ来ない（呼び出し元でfediのみに絞っている）が、
        // 念のための防御。
        return Ok(());
    }
    let Some(owner_ap_uri) = owner.ap_uri.clone() else {
        return Ok(());
    };

    let local_domain = ctx
        .inbox
        .as_ref()
        .map(|i| i.local_domain.as_str())
        .or_else(|| ctx.delivery.as_ref().map(|d| d.local_domain.as_str()));

    let owner_domain = owner_ap_uri.split('/').nth(2).unwrap_or("").to_string();
    let owner_ap = {
        let sem = ctx.get_domain_semaphore(&owner_domain).await;
        let _permit = sem
            .acquire_owned()
            .await
            .map_err(|e| format!("セマフォ取得失敗: {}", e))?;
        ctx.ap_client
            .fetch_actor(&owner_ap_uri)
            .await
            .map_err(|e| format!("移転元アクター取得失敗: {}", e))?
    };

    let mut target_ids: Vec<i64> = Vec::new();
    for uri in &owner_ap.also_known_as {
        match resolve_also_known_as_uri(&actors, &ctx, local_domain, uri).await {
            Ok(Some(id)) if id != owner_actor_id => target_ids.push(id),
            Ok(_) => {}
            Err(e) => tracing::info!("[RemoteAlsoKnownAsSync] {} の解決失敗: {}", uri, e),
        }
    }
    target_ids.sort_unstable();
    target_ids.dedup();

    let now = chrono::Utc::now();
    also_known_as
        .sync_remote_owner_targets(owner_actor_id, &target_ids, now)
        .await
        .map_err(|e| format!("同期失敗: {}", e))?;

    for target_actor_id in target_ids {
        if let Err(e) = ctx
            .queue
            .enqueue(
                Job::AlsoKnownAsVerify {
                    owner_actor_id,
                    target_actor_id,
                },
                priority::LOW,
            )
            .await
        {
            tracing::error!(
                "[RemoteAlsoKnownAsSync] AlsoKnownAsVerify enqueue失敗: {}",
                e
            );
        }
    }

    Ok(())
}

/// `alsoKnownAs`の1エントリ（URI）を解決してactor_idを返す。自ドメインはローカルDB参照、
/// `did:`はBsky、それ以外の`https://`はFediアクターとして解決・upsertする
/// （`jobs::remote_actor_resolve`と同様の「フォロー関係は作らずactorsへ反映するだけ」方針）。
async fn resolve_also_known_as_uri(
    actors: &dyn ActorRepository,
    ctx: &JobContext,
    local_domain: Option<&str>,
    uri: &str,
) -> Result<Option<i64>, String> {
    if let Some(local_domain) = local_domain {
        if let Some(username) = crate::ap::extract_local_username(uri, local_domain) {
            return actors
                .find_by_username_domain(username, local_domain)
                .await
                .map(|opt| opt.map(|a| a.id))
                .map_err(|e| format!("DB検索失敗: {}", e));
        }
    }

    if let Some(did) = uri.strip_prefix("did:").map(|_| uri) {
        let profile = crate::atp::fetch_bsky_profile(&ctx.ap_client.http, did)
            .await
            .map_err(|e| format!("Bskyプロフィール取得失敗: {}", e))?;
        let now = chrono::Utc::now();
        let new_id = generate_snowflake_id(now);
        let id = actors
            .upsert_remote_bsky(
                new_id,
                &profile.did,
                &profile.handle,
                profile.display_name.as_deref(),
                profile.avatar.as_deref(),
                now,
            )
            .await
            .map_err(|e| format!("upsert失敗: {}", e))?;
        return Ok(Some(id));
    }

    if uri.starts_with("https://") || uri.starts_with("http://") {
        if let Some(existing) = actors
            .find_by_ap_uri(uri)
            .await
            .map_err(|e| format!("DB検索失敗: {}", e))?
        {
            return Ok(Some(existing.id));
        }

        let domain = uri.split('/').nth(2).unwrap_or("").to_string();
        let sem = ctx.get_domain_semaphore(&domain).await;
        let remote_ap = {
            let _permit = sem
                .acquire_owned()
                .await
                .map_err(|e| format!("セマフォ取得失敗: {}", e))?;
            ctx.ap_client
                .fetch_actor(uri)
                .await
                .map_err(|e| format!("アクター取得失敗: {}", e))?
        };
        let Some(inbox) = remote_ap.inbox.clone() else {
            return Ok(None);
        };
        let username = remote_ap
            .preferred_username
            .clone()
            .ok_or_else(|| "preferredUsernameがありません".to_string())?;
        let display_name = remote_ap.name.clone().unwrap_or_else(|| username.clone());
        let bio = remote_ap
            .summary
            .as_deref()
            .map(crate::jobs::inbound_activity_process::strip_html);
        let emoji_map = remote_ap.emoji_map();
        let profile_fields = remote_ap.profile_fields_json();
        let now = chrono::Utc::now();
        let new_id = generate_snowflake_id(now);
        let id = actors
            .upsert_remote_fedi(
                new_id,
                uri,
                &inbox,
                &username,
                &domain,
                &display_name,
                remote_ap.avatar_url().as_deref(),
                bio.as_deref(),
                now,
                &emoji_map,
                &profile_fields,
            )
            .await
            .map_err(|e| format!("upsert失敗: {}", e))?;
        return Ok(Some(id));
    }

    Ok(None)
}

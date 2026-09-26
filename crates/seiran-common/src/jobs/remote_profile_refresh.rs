//! リモートアクター（fedi/bsky/remote_seiran）のプロフィール（avatar_url/banner_url/
//! display_name/bio等）再取得ジョブ。
//!
//! アクターの`avatar_url`/`banner_url`等は初回発見時（`upsert_remote_fedi`/
//! `upsert_remote_bsky`の新規行作成、または`resolve_bsky`のフォロー解決）にしか
//! 更新されない設計だったため、一度DBに登録された既存アクターがリモート側で
//! プロフィール画像を変更しても反映されなかった。`handlers::users::user_profile`が
//! DB登録済みアクターのプロフィール表示のたびに積み、表示自体は常にDB上の既存値を
//! そのまま返す（`jobs::remote_featured_sync`と同じ「表示時再検証」パターン）。

use std::sync::Arc;

use crate::queue::worker::JobContext;
use crate::repository::{ActorRepository, PgActorRepository};

pub async fn handle(actor_id: i64, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!("[RemoteProfileRefresh] DB pool 未設定のためスキップ");
        return Ok(());
    };
    let actors = PgActorRepository::new(pool.clone());

    let Some(actor) = actors
        .find_by_id(actor_id)
        .await
        .map_err(|e| format!("アクター取得失敗: {}", e))?
    else {
        return Ok(());
    };

    // fedi/remote_seiranはap_uri、bsky/remote_seiranはat_didを持つ。remote_seiranは
    // 両実体を持つため両方リフレッシュする。
    if let Some(ap_uri) = actor.ap_uri.clone() {
        if let Err(e) = refresh_fedi(&actors, &ap_uri, &ctx).await {
            tracing::info!(
                "[RemoteProfileRefresh] fedi再取得失敗（スキップ）: actor_id={} {}",
                actor_id,
                e
            );
        }
    }

    if let Some(at_did) = actor.at_did.clone() {
        if let Err(e) = refresh_bsky(&actors, &at_did, &ctx).await {
            tracing::info!(
                "[RemoteProfileRefresh] bsky再取得失敗（スキップ）: actor_id={} {}",
                actor_id,
                e
            );
        }
    }

    Ok(())
}

fn extract_domain(uri: &str) -> String {
    uri.strip_prefix("https://")
        .or_else(|| uri.strip_prefix("http://"))
        .and_then(|s| s.split('/').next())
        .unwrap_or("unknown")
        .to_string()
}

async fn refresh_fedi(
    actors: &PgActorRepository,
    ap_uri: &str,
    ctx: &JobContext,
) -> Result<(), String> {
    let domain = extract_domain(ap_uri);
    let sem = ctx.get_domain_semaphore(&domain).await;
    let _permit = sem
        .acquire_owned()
        .await
        .map_err(|e| format!("セマフォ取得失敗: {}", e))?;

    let ap_actor = ctx
        .ap_client
        .fetch_actor_with_key(
            ap_uri,
            crate::ap::client::signing_key_refs(&ctx.system_signing_key()),
        )
        .await
        .map_err(|e| format!("アクタードキュメント取得失敗: {}", e))?;

    let profile = crate::repository::FediActorProfile::from_ap_actor(&ap_actor, ap_uri, None)
        .map_err(|e| e.to_string())?;
    actors
        .upsert_remote_fedi(
            crate::generate_snowflake_id(chrono::Utc::now()),
            &profile,
            chrono::Utc::now(),
        )
        .await
        .map(|_| ())
        .map_err(|e| format!("upsert_remote_fedi失敗: {}", e))
}

async fn refresh_bsky(
    actors: &PgActorRepository,
    at_did: &str,
    ctx: &JobContext,
) -> Result<(), String> {
    let profile = crate::atp::fetch_bsky_profile(&ctx.ap_client.http, at_did).await?;
    let new_id = crate::generate_snowflake_id(chrono::Utc::now());
    actors
        .upsert_remote_bsky(
            new_id,
            &crate::repository::BskyActorProfile {
                at_did: &profile.did,
                handle: &profile.handle,
                display_name: profile.display_name.as_deref(),
                avatar_url: profile.avatar.as_deref(),
                banner_url: profile.banner.as_deref(),
            },
            chrono::Utc::now(),
        )
        .await
        .map(|_| ())
        .map_err(|e| format!("upsert_remote_bsky失敗: {}", e))
}

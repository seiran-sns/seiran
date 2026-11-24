//! リモートアクター（fedi/bsky/remote_seiran）のプロフィール（avatar_url/banner_url/
//! display_name/bio等）再取得ジョブ。
//!
//! アクターの`avatar_url`/`banner_url`等は、アクターを発見した経路が保存した時点の値のまま
//! なので、リモート側でプロフィール画像を変えても、このジョブが取り直すまで反映されない。
//! `handlers::users::user_profile`がDB登録済みアクターのプロフィール表示のたびに積み、
//! 表示自体は常にDB上の既存値をそのまま返す（`jobs::remote_featured_sync`と同じ
//! 「表示時再検証」パターン）。

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
        if let Err(e) = refresh_fedi(pool, &ap_uri, &ctx).await {
            tracing::info!(
                "[RemoteProfileRefresh] fedi再取得失敗（スキップ）: actor_id={} {}",
                actor_id,
                e
            );
        }
    }

    if let Some(at_did) = actor.at_did.clone() {
        if let Err(e) = refresh_bsky(pool, &at_did, &ctx).await {
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

async fn refresh_fedi(pool: &sqlx::PgPool, ap_uri: &str, ctx: &JobContext) -> Result<(), String> {
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
    crate::seiran_actor_merge::discover_fedi_profile(pool, ctx.queue.as_ref(), &profile, true)
        .await
        .map(|_| ())
        .map_err(|e| format!("discover_fedi_profile失敗: {}", e))
}

async fn refresh_bsky(pool: &sqlx::PgPool, at_did: &str, ctx: &JobContext) -> Result<(), String> {
    let profile = crate::atp::fetch_bsky_profile(&ctx.ap_client.http, at_did).await?;
    crate::seiran_actor_merge::discover_bsky_profile(
        pool,
        ctx.queue.as_ref(),
        &(&profile).into(),
        crate::seiran_actor_merge::DeclarationLookup::Always,
        true,
    )
    .await
    .map(|_| ())
    .map_err(|e| format!("discover_bsky_profile失敗: {}", e))
}

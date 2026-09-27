//! ④ アクター検証・メタデータ取得キュー (`actor_metadata_resolve`)
//!
//! リモートseiranアクターの相互申告マージ（#236）における「相手を能動的に取りに行く」
//! ジョブ。AP側/ATP側のどちらかを発見し、まだ相互一致が確認できていない
//! （`claimed_ap_uri`/`claimed_at_did`が設定されたままの）アクター行に対して積まれ、
//! 相手側の実体を能動的に解決することで結婚（マージ）成立を早める。成立の必須条件では
//! なく、通常の受動的発見（相手からの投稿受信・フォロー等）でも同じ判定が働く。
//! 詳細は`docs/protocols.md` 11節参照。

use std::sync::Arc;

use crate::generate_snowflake_id;
use crate::queue::worker::JobContext;
use crate::repository::{Actor, ActorRepository, PgActorRepository};

pub async fn handle(actor_id: i64, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!(
            "[ActorMetadataResolve] DB pool 未設定のためスキップ (actor_id={})",
            actor_id
        );
        return Ok(());
    };

    let actor_repo = PgActorRepository::new(pool.clone());
    let Some(actor) = actor_repo
        .find_by_id(actor_id)
        .await
        .map_err(|e| format!("DB検索失敗: {}", e))?
    else {
        return Ok(());
    };

    match actor.actor_type.as_str() {
        "fedi" => resolve_counterpart_via_atp(pool, &ctx, &actor).await,
        "bsky" => resolve_counterpart_via_ap(pool, &ctx, &actor).await,
        _ => Ok(()),
    }
}

/// `fedi`型行が自己申告する`claimed_at_did`の実体（Bskyプロフィール）を取りに行く。
async fn resolve_counterpart_via_atp(
    pool: &sqlx::PgPool,
    ctx: &JobContext,
    actor: &Actor,
) -> Result<(), String> {
    let Some(did) = actor.claimed_at_did.as_deref() else {
        return Ok(());
    };
    let profile = crate::atp::client::fetch_bsky_profile(&ctx.ap_client.http, did)
        .await
        .map_err(|e| format!("Bskyプロフィール取得失敗: {}", e))?;
    // 相手側解決ジョブ自身からの呼び出しのため、再投入は抑止する（無限ジョブ再投入対策）。
    crate::seiran_actor_merge::discover_bsky_profile(pool, ctx.queue.as_ref(), &profile, false)
        .await
        .map_err(|e| format!("discover_bsky_actor 失敗: {}", e))?;
    Ok(())
}

/// `bsky`型行が自己申告する`claimed_ap_uri`の実体（AP Actor文書）を取りに行く。
async fn resolve_counterpart_via_ap(
    pool: &sqlx::PgPool,
    ctx: &JobContext,
    actor: &Actor,
) -> Result<(), String> {
    let Some(ap_uri) = actor.claimed_ap_uri.as_deref() else {
        return Ok(());
    };
    // Authorized Fetch（secure mode）対応。署名鍵が組み立てられない場合のみ未署名フェッチへ
    // フォールバックする（`RemoteActorResolve`と同じパターン）。
    let remote_ap = ctx
        .ap_client
        .fetch_actor_with_key(
            ap_uri,
            crate::ap::client::signing_key_refs(&ctx.system_signing_key()),
        )
        .await
        .map_err(|e| format!("アクタードキュメント取得失敗: {}", e))?;
    // AP Actor文書自身が`seiranAtDid`拡張で自己申告する相手（`profile.claimed_at_did`）を使う。
    // 呼び出し元自身の`actor.at_did`を渡すと、`discover_fedi_actor`が自己参照で常に真の一致判定を
    // してしまい、取得したAP Actor文書が実際に何を自己申告しているか（そもそも`seiranAtDid`を
    // 持たない場合すら）を一切確認せず結婚が成立してしまう。
    let profile = match crate::repository::FediActorProfile::from_ap_actor(&remote_ap, ap_uri, None)
    {
        Ok(profile) => profile,
        Err(crate::ap::client::FediProfileError::MissingInbox) => return Ok(()),
        Err(e) => return Err(format!("{} ({})", e, ap_uri)),
    };
    let new_id = generate_snowflake_id(chrono::Utc::now());
    let outcome =
        crate::seiran_actor_merge::discover_fedi_actor(pool, new_id, &profile, chrono::Utc::now())
            .await
            .map_err(|e| format!("discover_fedi_actor 失敗: {}", e))?;
    // `claimed`にNoneを渡し、再enqueueは抑止する（`resolve_counterpart_via_atp`と同じ
    // 無限ジョブ再投入対策）。
    crate::seiran_actor_merge::promote_after_discovery(pool, ctx.queue.as_ref(), &outcome, None)
        .await;
    Ok(())
}

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
    // DID側が`org.seiran.actor.declaration`で自己申告する相手（AP Actor URI）を取りに行く。
    // 呼び出し元自身の`actor.ap_uri`を渡すと、`discover_bsky_actor`が自己参照で常に真の
    // 一致判定をしてしまい、DID側の独立した自己申告を一切確認せず結婚が成立してしまう
    // （実地検証で発覚。firehose.rsのATP先着経路と同じ取得元に揃える）。
    let claimed_ap_uri = crate::atp::client::fetch_seiran_actor_declaration(did).await;
    let new_id = generate_snowflake_id(chrono::Utc::now());
    let outcome = crate::seiran_actor_merge::discover_bsky_actor(
        pool,
        new_id,
        did,
        &profile.handle,
        profile.display_name.as_deref(),
        profile.avatar.as_deref(),
        claimed_ap_uri.as_deref(),
        chrono::Utc::now(),
    )
    .await
    .map_err(|e| format!("discover_bsky_actor 失敗: {}", e))?;
    if outcome.married {
        // 結婚成立でこの行に初めてat_didが載る。既にこの行をフォロー中のローカル
        // ユーザーがいてもJetstreamのwanted_didsは自動で追随しないため、ここで
        // 明示的に再構築を促す（実地検証で発覚。`follow_exec::follow_fedi`の
        // 同種コメント参照）。
        crate::jetstream_control::touch_jetstream_wanted_dids(pool).await;
    }
    // 結婚不成立でもここでは再enqueueしない（相手側が能動的に取りに来る、または
    // 通常の受動的発見に任せる。無限ジョブ再投入を避けるため）。
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
    let remote_ap = match ctx.system_signing_key() {
        Some((key_id, pem)) => ctx
            .ap_client
            .fetch_actor_signed(ap_uri, (&key_id, &pem))
            .await
            .map_err(|e| format!("アクタードキュメント取得失敗: {}", e))?,
        None => ctx
            .ap_client
            .fetch_actor(ap_uri)
            .await
            .map_err(|e| format!("アクタードキュメント取得失敗: {}", e))?,
    };
    let Some(ap_inbox) = remote_ap.inbox.clone() else {
        return Ok(());
    };
    let username = remote_ap.preferred_username.clone().ok_or_else(|| {
        format!(
            "リモートアクター '{}' に preferredUsername がありません",
            ap_uri
        )
    })?;
    let display_name = remote_ap.name.clone().unwrap_or_else(|| username.clone());
    let domain = ap_uri.split('/').nth(2).unwrap_or("").to_string();
    let avatar_url = remote_ap.avatar_url();
    let bio = remote_ap
        .summary
        .as_deref()
        .map(crate::jobs::inbound_activity_process::strip_html);
    let emoji_map = remote_ap.emoji_map();
    let profile_fields = remote_ap.profile_fields_json();

    // AP Actor文書自身が`seiranAtDid`拡張で自己申告する相手を使う。呼び出し元自身の
    // `actor.at_did`を渡すと、`discover_fedi_actor`が自己参照で常に真の一致判定をしてしまい、
    // 取得したAP Actor文書が実際に何を自己申告しているか（そもそも`seiranAtDid`を
    // 持たない場合すら）を一切確認せず結婚が成立してしまう（実地検証で発覚）。
    let new_id = generate_snowflake_id(chrono::Utc::now());
    let outcome = crate::seiran_actor_merge::discover_fedi_actor(
        pool,
        new_id,
        ap_uri,
        &ap_inbox,
        &username,
        &domain,
        &display_name,
        avatar_url.as_deref(),
        bio.as_deref(),
        &emoji_map,
        &profile_fields,
        remote_ap.seiran_at_did.as_deref(),
        chrono::Utc::now(),
    )
    .await
    .map_err(|e| format!("discover_fedi_actor 失敗: {}", e))?;
    if outcome.married {
        crate::jetstream_control::touch_jetstream_wanted_dids(pool).await;
    }
    Ok(())
}

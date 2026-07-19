//! ⑦ Bsky メンション DID 解決キュー (`resolve_bsky_mention`)
//!
//! Bluesky投稿のメンションfacetに含まれるDIDが、まだローカルの `actors` テーブルに
//! 存在しない場合に非同期で AppView 解決してupsertする（`resolve_or_upsert_bsky_actor`
//! と同じロジック、いいね受信時にも同種の解決が走るためそちらで既に解決済みなら何もしない）。
//! `NoteResponse` 生成時点（`crates/seiran-api/src/handlers/notes/dto.rs`）で未解決の
//! メンションは元のテキストのまま返され、このジョブが解決を終えた後の次回表示から
//! `@handle.domain` に変換される。

use std::sync::Arc;

use crate::atp::fetch_bsky_profile;
use crate::generate_snowflake_id;
use crate::queue::worker::JobContext;
use crate::repository::{ActorRepository, PgActorRepository};

pub async fn handle(did: String, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!("[ResolveBskyMention] DB pool 未設定のためスキップ (did={})", did);
        return Ok(());
    };

    let actor_repo = PgActorRepository::new(pool.clone());
    if actor_repo
        .find_by_did(&did)
        .await
        .map_err(|e| format!("DB検索失敗: {}", e))?
        .is_some()
    {
        // 既に他経路（いいね受信等）で解決済み。
        return Ok(());
    }

    let profile = fetch_bsky_profile(&ctx.ap_client.http, &did).await?;
    let new_id = generate_snowflake_id(chrono::Utc::now());
    actor_repo
        .upsert_remote_bsky(
            new_id,
            &did,
            &profile.handle,
            profile.display_name.as_deref(),
            profile.avatar.as_deref(),
            chrono::Utc::now(),
        )
        .await
        .map_err(|e| format!("upsert_remote_bsky 失敗: {}", e))?;

    tracing::info!(
        "[ResolveBskyMention] 解決完了: did={} handle={}",
        did, profile.handle
    );
    Ok(())
}

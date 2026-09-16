//! Fedi受信DM（`visibility='direct'`）の`to`に含まれるリモートアクターURIを解決し
//! `post_recipients`へ追加するジョブ（`Job::DmRecipientResolve`）。
//!
//! DM受信処理（`jobs::inbound_activity_process::note_save`）自体は`to`のうちローカル
//! アクターのみを即座に`post_recipients`へ解決する（リモートは都度フェッチが必要になり
//! 受信処理をブロックしたくないため）。3人以上の会話にリモートユーザーが混じる場合、
//! そのままだと宛先表示（`docs/ui_spec.md` 2.5節）から漏れてしまうため、未知アクターも
//! `jobs::remote_actor_resolve::resolve_and_upsert`と同じ要領で解決してから積む。

use std::sync::Arc;

use crate::queue::worker::JobContext;

pub async fn handle(post_id: i64, uri: String, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!(
            "[DmRecipientResolve] DB pool 未設定のためスキップ (post_id={})",
            post_id
        );
        return Ok(());
    };

    let Some(actor_id) = super::remote_actor_resolve::resolve_and_upsert(&uri, &ctx).await? else {
        return Ok(());
    };

    sqlx::query(
        "INSERT INTO post_recipients (post_id, actor_id) VALUES ($1, $2)
         ON CONFLICT (post_id, actor_id) DO NOTHING",
    )
    .bind(post_id)
    .bind(actor_id)
    .execute(pool)
    .await
    .map_err(|e| format!("post_recipients INSERT失敗: {}", e))?;

    tracing::info!(
        "[DmRecipientResolve] 宛先解決完了: post_id={} uri={} actor_id={}",
        post_id,
        uri,
        actor_id
    );
    Ok(())
}

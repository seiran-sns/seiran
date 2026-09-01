//! リモートFediアクターのfeatured collection（ピン留め投稿, #61）同期ジョブ。
//!
//! DB登録済みアクターのプロフィール表示のたびに積まれ、表示自体は常にDB上の既存
//! `pinned_posts`をそのまま返す（`jobs::also_known_as_verify`と同じ「表示時再検証」
//! パターン）。Authorized Fetch（secure mode）を要求するリモートだと同期フェッチが
//! 数秒かかることがあり、プロフィール表示のたびにブロッキングで待つのは体感速度を
//! 損なうため、`handlers::users::sync_remote_fedi_pinned`（旧同期実装）から切り出した。

use std::sync::Arc;

use crate::queue::worker::JobContext;
use crate::repository::{ActorRepository, PgActorRepository, PgPinnedPostsRepository};

pub async fn handle(actor_id: i64, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!("[RemoteFeaturedSync] DB pool 未設定のためスキップ");
        return Ok(());
    };
    let actors = PgActorRepository::new(pool.clone());
    let pinned_posts = PgPinnedPostsRepository::new(pool.clone());

    let Some(actor) = actors
        .find_by_id(actor_id)
        .await
        .map_err(|e| format!("アクター取得失敗: {}", e))?
    else {
        return Ok(());
    };
    let Some(ap_uri) = actor.ap_uri.clone() else {
        return Ok(());
    };

    let signing_key = ctx.system_signing_key();
    let notes = match crate::ap::fetch_ap_featured(
        &ctx.ap_client,
        &ap_uri,
        signing_key.as_ref().map(|(k, p)| (k.as_str(), p.as_str())),
    )
    .await
    {
        Ok(notes) => notes,
        Err(e) => {
            tracing::info!(
                "[RemoteFeaturedSync] featured collection 取得失敗（スキップ）: actor_id={} {}",
                actor_id,
                e
            );
            return Ok(());
        }
    };

    let mut post_ids = Vec::with_capacity(notes.len());
    for note in &notes {
        match crate::ap::upsert_ap_note(pool, actor_id, note).await {
            Ok(id) => post_ids.push(id),
            Err(e) => tracing::warn!(
                "[RemoteFeaturedSync] featured Note 保存失敗（スキップ）: {}",
                e
            ),
        }
    }

    use crate::repository::PinnedPostsRepository;
    pinned_posts
        .sync_from_remote(actor_id, &post_ids, chrono::Utc::now())
        .await
        .map_err(|e| format!("pinned_posts 同期失敗: {}", e))?;

    Ok(())
}

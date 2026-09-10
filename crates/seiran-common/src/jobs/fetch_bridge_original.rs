//! `Job::FetchBridgeOriginal`（`crate::bridge_post`参照）: brid.gy(Bridgy Fed)ブリッジポスト
//! 取り込み時点で元ポストがDB未登録だった場合に、元ポストを能動的に取得しに行く。
//!
//! 保存自体は各プロトコルの既存パイプラインへ委譲する（`atp::upsert_bsky_post`/
//! `inbound_activity_process::reference::save_fetched_remote_note`）。保存後は挿入経路側の
//! `bridge_post::link_pending_bridges_for_new_original`が自動的に走り、この`bridge_post_id`と
//! 結合される。失敗時は有限回リトライ後あきらめてよい（元ポストが後で通常の受信経路で届けば、
//! 受動的リンクが安全網になる）。

use std::sync::Arc;

use crate::atp::{fetch_single_bsky_post, upsert_bsky_post};
use crate::jobs::inbound_activity_process::reference::{
    save_fetched_remote_note, system_signing_key,
};
use crate::queue::worker::JobContext;
use crate::repository::{ActorRepository, PgActorRepository};

pub async fn handle(
    bridge_post_id: i64,
    target_uri: String,
    protocol: String,
    ctx: Arc<JobContext>,
) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!(
            "[FetchBridgeOriginal] DB pool 未設定のためスキップ (bridge_post_id={})",
            bridge_post_id
        );
        return Ok(());
    };

    match protocol.as_str() {
        "atp" => fetch_atp_original(pool, &ctx, &target_uri).await,
        "ap" => fetch_ap_original(&ctx, &target_uri).await,
        other => {
            tracing::warn!("[FetchBridgeOriginal] 未知のprotocol: {}", other);
            Ok(())
        }
    }
}

async fn fetch_atp_original(
    pool: &sqlx::PgPool,
    ctx: &Arc<JobContext>,
    at_uri: &str,
) -> Result<(), String> {
    let http = ctx.ap_client.http.as_ref();
    let post = match fetch_single_bsky_post(http, at_uri).await {
        Ok(Some(post)) => post,
        Ok(None) => {
            tracing::info!(
                "[FetchBridgeOriginal] 元ATP投稿が見つかりません（諦めます） at_uri={}",
                at_uri
            );
            return Ok(());
        }
        Err(e) => return Err(format!("元ATP投稿の取得失敗: {}", e)),
    };

    let actors = PgActorRepository::new(pool.clone());
    let actor_id = match actors.find_by_did(&post.author_did).await {
        Ok(Some(actor)) => actor.id,
        Ok(None) => actors
            .upsert_remote_bsky(
                crate::generate_snowflake_id(chrono::Utc::now()),
                &post.author_did,
                &post.author_handle,
                post.author_display_name.as_deref(),
                post.author_avatar.as_deref(),
                chrono::Utc::now(),
            )
            .await
            .map_err(|e| format!("元投稿アクターupsert失敗: {}", e))?,
        Err(e) => return Err(format!("元投稿アクター検索失敗: {}", e)),
    };

    upsert_bsky_post(pool, &ctx.queue, http, actor_id, &post)
        .await
        .map_err(|e| format!("元ATP投稿の保存失敗: {}", e))?;
    Ok(())
}

async fn fetch_ap_original(ctx: &Arc<JobContext>, ap_uri: &str) -> Result<(), String> {
    let Some(inbox) = &ctx.inbox else {
        tracing::warn!(
            "[FetchBridgeOriginal] InboxContext 未設定のためスキップ（federationロール以外）"
        );
        return Ok(());
    };

    // ループバック/既存重複チェック（1段階フェッチ制限と同じ理由でDB照合を先に行う）。
    if let Ok(Some(_)) = inbox.post_repo.find_id_by_ap_or_at_uri(ap_uri).await {
        return Ok(());
    }

    let signing_key = system_signing_key(inbox);
    let note = match ctx
        .ap_client
        .fetch_object(ap_uri, (&signing_key.0, &signing_key.1))
        .await
    {
        Ok(note) => note,
        Err(crate::ap::ApError::Gone(detail)) => {
            tracing::info!(
                "[FetchBridgeOriginal] 元AP投稿が消失（諦めます） uri={}: {}",
                ap_uri,
                detail
            );
            return Ok(());
        }
        Err(e) => return Err(format!("元AP投稿の取得失敗: {}", e)),
    };

    save_fetched_remote_note(note, inbox, &ctx.ap_client)
        .await
        .map(|_| ())
}

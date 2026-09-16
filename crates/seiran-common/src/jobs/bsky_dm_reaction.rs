//! bsky宛DMメッセージへの絵文字リアクション付与/取消・「隠す」をBluesky公式チャット
//! サービスへ配送するジョブ（`Job::BskyDmReactionAdd`/`BskyDmReactionRemove`/`BskyDmHide`）。
//!
//! ローカルDBへの保存（`dm_bsky_reactions`/`dm_hidden_messages`）はAPIハンドラ側で
//! 同期的に行い（`handlers::dm_bsky::create_dm_bsky_reaction`等）、ここではBluesky側への
//! 反映（`chat.bsky.convo.addReaction`/`removeReaction`/`deleteMessageForSelf`）のみを扱う。
//! 認証は`bsky_dm_send`と同じ自己署名サービス認証JWT。

use std::sync::Arc;

use crate::atp::sign_service_auth_jwt;
use crate::queue::worker::JobContext;
use crate::repository::{DmRepository, PgDmRepository};

use super::bsky_dm_send::{CHAT_SERVICE_AUD, CHAT_SERVICE_HOST};

async fn send_action(
    ctx: &Arc<JobContext>,
    pool: &sqlx::PgPool,
    post_id: i64,
    actor_id: i64,
    xrpc_path: &str,
    extra_body: serde_json::Value,
    log_label: &str,
) -> Result<(), String> {
    let dm = PgDmRepository::new(pool.clone());
    let Some(context) = dm
        .bsky_dm_context(post_id, actor_id)
        .await
        .map_err(|e| format!("bsky_dm_context取得失敗: {}", e))?
    else {
        tracing::info!(
            "[{}] post_id={} はbsky宛DMではない（終了）",
            log_label,
            post_id
        );
        return Ok(());
    };
    let Some(message_id) = context.bsky_message_id else {
        tracing::warn!(
            "[{}] post_id={} にbsky_message_idが無い（終了、送信直後の反映待ちの可能性）",
            log_label,
            post_id
        );
        return Ok(());
    };

    let jwt = sign_service_auth_jwt(
        &context.viewer_pem,
        &context.viewer_did,
        CHAT_SERVICE_AUD,
        xrpc_path,
    )
    .map_err(|e| format!("JWT署名失敗: {}", e))?;

    let mut body = serde_json::json!({
        "convoId": context.convo_id,
        "messageId": message_id,
    });
    if let (Some(body_obj), Some(extra_obj)) = (body.as_object_mut(), extra_body.as_object()) {
        for (k, v) in extra_obj {
            body_obj.insert(k.clone(), v.clone());
        }
    }

    let resp = ctx
        .ap_client
        .http
        .post(format!("{}/xrpc/{}", CHAT_SERVICE_HOST, xrpc_path))
        .bearer_auth(&jwt)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("{}リクエスト失敗: {}", xrpc_path, e))?;

    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    if status.is_success() {
        tracing::info!(
            "[{}] 送信成功 post_id={} actor_id={}",
            log_label,
            post_id,
            actor_id
        );
        Ok(())
    } else if status.as_u16() == 400 {
        // ビジネスロジック拒否（相手側の会話が既に無い等）。リトライしても直らないため破棄する。
        tracing::warn!(
            "[{}] 送信拒否 post_id={} status={} body={}（リトライ対象外）",
            log_label,
            post_id,
            status,
            body_text
        );
        Ok(())
    } else {
        Err(format!(
            "{}失敗 status={} body={}",
            xrpc_path, status, body_text
        ))
    }
}

pub async fn handle_add(
    post_id: i64,
    actor_id: i64,
    content: String,
    ctx: Arc<JobContext>,
) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.clone() else {
        tracing::warn!("[BskyDmReactionAdd] DB pool 未設定のためスキップ");
        return Ok(());
    };
    send_action(
        &ctx,
        &pool,
        post_id,
        actor_id,
        "chat.bsky.convo.addReaction",
        serde_json::json!({ "value": content }),
        "BskyDmReactionAdd",
    )
    .await
}

pub async fn handle_remove(
    post_id: i64,
    actor_id: i64,
    content: String,
    ctx: Arc<JobContext>,
) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.clone() else {
        tracing::warn!("[BskyDmReactionRemove] DB pool 未設定のためスキップ");
        return Ok(());
    };
    send_action(
        &ctx,
        &pool,
        post_id,
        actor_id,
        "chat.bsky.convo.removeReaction",
        serde_json::json!({ "value": content }),
        "BskyDmReactionRemove",
    )
    .await
}

pub async fn handle_hide(post_id: i64, actor_id: i64, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.clone() else {
        tracing::warn!("[BskyDmHide] DB pool 未設定のためスキップ");
        return Ok(());
    };
    send_action(
        &ctx,
        &pool,
        post_id,
        actor_id,
        "chat.bsky.convo.deleteMessageForSelf",
        serde_json::json!({}),
        "BskyDmHide",
    )
    .await
}

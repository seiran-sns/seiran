//! ⑦ Bsky DM送信キュー (`bsky_dm_send`)
//!
//! DM（`visibility='direct'`）投稿の宛先にBskyアクターが含まれる場合、
//! `chat.bsky.convo.sendMessage` で実際にBluesky公式chatサービスへメッセージを送る。
//! 認証は自己署名サービス認証JWT（`docs/skill_atp_rust_programming.md` §17、
//! 2026-07-20実機疎通確認済み）。`aud`はfragment無しの`did:web:api.bsky.chat`を使うこと
//! （fragment込みだと`BadJwtAudience`で拒否される）。

use std::sync::Arc;

use sqlx::Row;

use crate::atp::sign_service_auth_jwt;
use crate::queue::worker::JobContext;

const CHAT_SERVICE_HOST: &str = "https://api.bsky.chat";
const CHAT_SERVICE_AUD: &str = "did:web:api.bsky.chat";

pub async fn handle(post_id: i64, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        tracing::warn!("[BskyDmSend] DB pool 未設定のためスキップ (post_id={})", post_id);
        return Ok(());
    };

    let row = sqlx::query(
        "SELECT p.body, p.thread_root_post_id, a.at_did AS sender_did, a.at_signing_key_pem AS sender_pem
         FROM posts p JOIN actors a ON a.id = p.actor_id
         WHERE p.id = $1 AND p.visibility = 'direct' LIMIT 1",
    )
    .bind(post_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("投稿情報取得失敗: {}", e))?;

    let Some(row) = row else {
        tracing::warn!("[BskyDmSend] post_id={} が見つかりません（終了）", post_id);
        return Ok(());
    };

    let body: String = row.try_get("body").map_err(|e| e.to_string())?;
    let thread_root_post_id: Option<i64> = row.try_get("thread_root_post_id").unwrap_or(None);
    let sender_did: Option<String> = row.try_get("sender_did").unwrap_or(None);
    let sender_pem: Option<String> = row.try_get("sender_pem").unwrap_or(None);

    let (Some(thread_root_post_id), Some(sender_did), Some(sender_pem)) =
        (thread_root_post_id, sender_did, sender_pem)
    else {
        tracing::error!("[BskyDmSend] post_id={} に必要な情報が無い（送信者のDID/署名鍵未設定、終了）", post_id);
        return Ok(());
    };

    let peer_row = sqlx::query(
        "SELECT a.at_did FROM post_recipients pr JOIN actors a ON a.id = pr.actor_id
         WHERE pr.post_id = $1 AND a.actor_type = 'bsky' AND a.at_did IS NOT NULL LIMIT 1",
    )
    .bind(post_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("宛先取得失敗: {}", e))?;

    let Some(peer_did) = peer_row.and_then(|r| r.try_get::<String, _>("at_did").ok()) else {
        tracing::info!("[BskyDmSend] post_id={} にBsky宛先が無い（終了）", post_id);
        return Ok(());
    };

    let convo_id = resolve_convo_id(&ctx, pool, thread_root_post_id, &sender_did, &sender_pem, &peer_did).await?;

    let jwt = sign_service_auth_jwt(&sender_pem, &sender_did, CHAT_SERVICE_AUD, "chat.bsky.convo.sendMessage")
        .map_err(|e| format!("JWT署名失敗: {}", e))?;

    let resp = ctx
        .ap_client
        .http
        .post(format!("{}/xrpc/chat.bsky.convo.sendMessage", CHAT_SERVICE_HOST))
        .bearer_auth(&jwt)
        .json(&serde_json::json!({
            "convoId": convo_id,
            "message": { "text": body },
        }))
        .send()
        .await
        .map_err(|e| format!("sendMessageリクエスト失敗: {}", e))?;

    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();

    if status.is_success() {
        tracing::info!("[BskyDmSend] 送信成功 post_id={} convo_id={}", post_id, convo_id);
        Ok(())
    } else if status.as_u16() == 400 {
        // ビジネスロジック拒否（受信者側のDM許可設定等、`docs/skill_atp_rust_programming.md` §17-3）。
        // リトライしても直らないため破棄する。
        tracing::warn!(
            "[BskyDmSend] 送信拒否 post_id={} status={} body={}（リトライ対象外）",
            post_id, status, body_text
        );
        Ok(())
    } else {
        Err(format!("sendMessage失敗 status={} body={}", status, body_text))
    }
}

/// `bsky_convo_links` にキャッシュがあればそれを使い、無ければ `getConvoForMembers` で
/// 1:1会話を解決してキャッシュする。
async fn resolve_convo_id(
    ctx: &Arc<JobContext>,
    pool: &sqlx::PgPool,
    thread_root_post_id: i64,
    sender_did: &str,
    sender_pem: &str,
    peer_did: &str,
) -> Result<String, String> {
    if let Some(cached) = sqlx::query_scalar::<_, String>(
        "SELECT convo_id FROM bsky_convo_links WHERE thread_root_post_id = $1",
    )
    .bind(thread_root_post_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("convoIdキャッシュ取得失敗: {}", e))?
    {
        return Ok(cached);
    }

    let jwt = sign_service_auth_jwt(sender_pem, sender_did, CHAT_SERVICE_AUD, "chat.bsky.convo.getConvoForMembers")
        .map_err(|e| format!("JWT署名失敗: {}", e))?;
    let url = format!(
        "{}/xrpc/chat.bsky.convo.getConvoForMembers?members={}&members={}",
        CHAT_SERVICE_HOST, sender_did, peer_did,
    );
    let resp = ctx
        .ap_client
        .http
        .get(&url)
        .bearer_auth(&jwt)
        .send()
        .await
        .map_err(|e| format!("getConvoForMembersリクエスト失敗: {}", e))?;

    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("getConvoForMembers失敗 status={} body={}", status, body_text));
    }

    let parsed: serde_json::Value =
        serde_json::from_str(&body_text).map_err(|e| format!("getConvoForMembers応答パース失敗: {}", e))?;
    let convo_id = parsed
        .get("convo")
        .and_then(|c| c.get("id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("getConvoForMembers応答にconvo.idが無い: {}", body_text))?;

    sqlx::query(
        "INSERT INTO bsky_convo_links (thread_root_post_id, convo_id) VALUES ($1, $2)
         ON CONFLICT (thread_root_post_id) DO UPDATE SET convo_id = EXCLUDED.convo_id",
    )
    .bind(thread_root_post_id)
    .bind(convo_id)
    .execute(pool)
    .await
    .map_err(|e| format!("convoIdキャッシュ保存失敗: {}", e))?;

    Ok(convo_id.to_string())
}

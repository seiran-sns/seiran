//! Bsky DM受信ポーリング。
//!
//! `chat.bsky.convo`はJetstreamに乗らない（私信のため公開ファイヤホースに含まれない）ため、
//! ローカルBskyリンク済みユーザーごとに `listConvos`/`getMessages` を定期ポーリングして
//! 新着メッセージを `posts`（visibility=direct）として取り込む。
//! 認証方式は `docs/skill_atp_rust_programming.md` §17 参照（自己署名サービス認証JWT）。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use seiran_common::atp::sign_service_auth_jwt;
use seiran_common::generate_snowflake_id;
use seiran_common::streaming::StreamHub;
use sqlx::{PgPool, Row};

use crate::firehose::resolve_or_upsert_bsky_actor;

const CHAT_SERVICE_HOST: &str = "https://api.bsky.chat";
const CHAT_SERVICE_AUD: &str = "did:web:api.bsky.chat";
const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// DM受信ポーリングを常駐実行する。
pub async fn run(pool: PgPool, http: Arc<reqwest::Client>, stream_hub: Arc<StreamHub>) {
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    loop {
        interval.tick().await;
        if let Err(e) = poll_once(&pool, &http, &stream_hub).await {
            tracing::error!("[BskyDmPoll] ポーリング失敗: {}", e);
        }
    }
}

async fn poll_once(pool: &PgPool, http: &reqwest::Client, stream_hub: &StreamHub) -> Result<(), String> {
    let users = sqlx::query(
        "SELECT id, at_did, at_signing_key_pem FROM actors
         WHERE actor_type = 'local' AND at_did IS NOT NULL AND at_signing_key_pem IS NOT NULL",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    for row in users {
        let actor_id: i64 = row.try_get("id").map_err(|e| e.to_string())?;
        let did: String = row.try_get("at_did").map_err(|e| e.to_string())?;
        let pem: String = row.try_get("at_signing_key_pem").map_err(|e| e.to_string())?;
        if let Err(e) = poll_user(pool, http, stream_hub, actor_id, &did, &pem).await {
            // 401は主にDIDがPLCディレクトリ上で無効（テスト用アカウント等）な場合に発生する
            // 想定内のケースのため warn 止まりとし、エラー監視のノイズにしない。
            tracing::warn!("[BskyDmPoll] actor_id={} のポーリング失敗: {}", actor_id, e);
        }
    }
    Ok(())
}

async fn poll_user(
    pool: &PgPool,
    http: &reqwest::Client,
    stream_hub: &StreamHub,
    actor_id: i64,
    did: &str,
    pem: &str,
) -> Result<(), String> {
    let jwt = sign_service_auth_jwt(pem, did, CHAT_SERVICE_AUD, "chat.bsky.convo.listConvos")
        .map_err(|e| e.to_string())?;
    let resp = http
        .get(format!("{}/xrpc/chat.bsky.convo.listConvos", CHAT_SERVICE_HOST))
        .bearer_auth(&jwt)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("listConvos失敗 status={}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let convos = body.get("convos").and_then(|v| v.as_array()).cloned().unwrap_or_default();

    for convo in &convos {
        if let Err(e) = sync_convo(pool, http, stream_hub, actor_id, did, pem, convo).await {
            tracing::error!("[BskyDmPoll] convo同期失敗 actor_id={}: {}", actor_id, e);
        }
    }
    Ok(())
}

async fn sync_convo(
    pool: &PgPool,
    http: &reqwest::Client,
    stream_hub: &StreamHub,
    local_actor_id: i64,
    local_did: &str,
    local_pem: &str,
    convo: &serde_json::Value,
) -> Result<(), String> {
    let convo_id = convo.get("id").and_then(|v| v.as_str()).ok_or("convo.idが無い")?;
    // グループ会話（`groupConvo`）は対象外。1:1のみ取り込む。
    let kind = convo.get("kind").and_then(|v| v.as_str()).unwrap_or("directConvo");
    if kind != "directConvo" {
        return Ok(());
    }

    let members = convo.get("members").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let peer_did = members
        .iter()
        .filter_map(|m| m.get("did").and_then(|v| v.as_str()))
        .find(|d| *d != local_did)
        .map(|s| s.to_string());
    let Some(peer_did) = peer_did else { return Ok(()) };

    let existing = sqlx::query(
        "SELECT thread_root_post_id, last_synced_message_id FROM bsky_convo_links WHERE convo_id = $1",
    )
    .bind(convo_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut thread_root_post_id: Option<i64> = None;
    let mut last_synced: Option<String> = None;
    if let Some(row) = existing {
        thread_root_post_id = row.try_get("thread_root_post_id").ok();
        last_synced = row.try_get::<Option<String>, _>("last_synced_message_id").ok().flatten();
    }

    let jwt = sign_service_auth_jwt(local_pem, local_did, CHAT_SERVICE_AUD, "chat.bsky.convo.getMessages")
        .map_err(|e| e.to_string())?;
    let resp = http
        .get(format!("{}/xrpc/chat.bsky.convo.getMessages?convoId={}", CHAT_SERVICE_HOST, convo_id))
        .bearer_auth(&jwt)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("getMessages失敗 status={}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let messages = body.get("messages").and_then(|v| v.as_array()).cloned().unwrap_or_default();

    // getMessagesは新しい順で返る。前回同期済みのメッセージIDに到達するまでを「新着」として集める。
    let mut new_messages: Vec<serde_json::Value> = Vec::new();
    for m in &messages {
        let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if Some(id.to_string()) == last_synced {
            break;
        }
        new_messages.push(m.clone());
    }
    if new_messages.is_empty() {
        return Ok(());
    }
    new_messages.reverse(); // 古い順に処理する

    let peer_actor_id = resolve_or_upsert_bsky_actor(pool, http, &peer_did).await?;

    let mut current_thread_root = thread_root_post_id;

    // メッセージ1件ごとに「posts INSERT + post_recipients INSERT + カーソル前進」を
    // 単一トランザクションでコミットする。複数メッセージの取り込み途中でエラーが起きても、
    // 既にコミット済みの分のカーソルは進んでいるため、次回ポーリングでの再取り込みは
    // 未コミット分のみに限定される。bsky_message_id のUNIQUE制約（DO NOTHING）が
    // それでも起こりうる二重取り込み（同時ポーリング等）に対する保険になる。
    for m in &new_messages {
        let msg_id = m.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let sender_did = m.get("sender").and_then(|s| s.get("did")).and_then(|v| v.as_str()).unwrap_or_default();

        if sender_did == local_did {
            // 自分が送信したメッセージ（BskyDmSend経由で既にpostsに存在する）はスキップするが、
            // カーソルは進める必要がある。スレッド起点が未確定（＝この会話で送受信いずれの
            // メッセージもまだ一件もない）場合はbsky_convo_linksの行を作れないため据え置く。
            // その場合は次のメッセージ処理時、または次回ポーリングでの再スキップにより
            // いずれ解消される（実害のない読み飛ばし）。
            if let Some(thread_root) = current_thread_root {
                persist_cursor(pool, thread_root, convo_id, &msg_id).await?;
            }
            continue;
        }

        let text = m.get("text").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let sent_at = m
            .get("sentAt")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(chrono::Utc::now);

        let candidate_post_id = generate_snowflake_id(sent_at);
        let candidate_thread_root = current_thread_root.unwrap_or(candidate_post_id);

        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

        let inserted_id: Option<i64> = sqlx::query_scalar(
            "INSERT INTO posts (id, actor_id, body, visibility, thread_root_post_id, created_at, bsky_message_id)
             VALUES ($1, $2, $3, 'direct', $4, $5, $6)
             ON CONFLICT (bsky_message_id) DO NOTHING
             RETURNING id",
        )
        .bind(candidate_post_id)
        .bind(peer_actor_id)
        .bind(&text)
        .bind(candidate_thread_root)
        .bind(sent_at)
        .bind(&msg_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| format!("DM受信INSERT失敗: {}", e))?;

        // ON CONFLICT で行が挿入されなかった場合は、以前のポーリングで既に取り込み済み。
        // その時に確定したpost_id/thread_rootを読み直し、今回生成した値は破棄する。
        let (actual_post_id, actual_thread_root) = match inserted_id {
            Some(id) => (id, candidate_thread_root),
            None => {
                let row = sqlx::query("SELECT id, thread_root_post_id FROM posts WHERE bsky_message_id = $1")
                    .bind(&msg_id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|e| format!("既存DM取得失敗: {}", e))?;
                let id: i64 = row.try_get("id").map_err(|e| e.to_string())?;
                let root: Option<i64> = row.try_get("thread_root_post_id").ok().flatten();
                (id, root.unwrap_or(id))
            }
        };

        sqlx::query("INSERT INTO post_recipients (post_id, actor_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(actual_post_id)
            .bind(local_actor_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("post_recipients INSERT失敗: {}", e))?;

        sqlx::query(
            "INSERT INTO bsky_convo_links (thread_root_post_id, convo_id, last_synced_message_id)
             VALUES ($1, $2, $3)
             ON CONFLICT (thread_root_post_id) DO UPDATE SET last_synced_message_id = EXCLUDED.last_synced_message_id",
        )
        .bind(actual_thread_root)
        .bind(convo_id)
        .bind(&msg_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        tx.commit().await.map_err(|e| e.to_string())?;

        current_thread_root = Some(actual_thread_root);

        let peer_row = sqlx::query("SELECT username, domain, display_name, avatar_url FROM actors WHERE id = $1")
            .bind(peer_actor_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
        let (peer_username, peer_domain, peer_display_name, peer_avatar_url): (String, String, Option<String>, Option<String>) =
            match peer_row {
                Some(r) => (
                    r.try_get("username").unwrap_or_default(),
                    r.try_get("domain").unwrap_or_default(),
                    r.try_get("display_name").unwrap_or(None),
                    r.try_get("avatar_url").unwrap_or(None),
                ),
                None => (String::new(), String::new(), None, None),
            };

        let note_json = serde_json::json!({
            "id": actual_post_id.to_string(),
            "text": text,
            "createdAt": sent_at.to_rfc3339(),
            "user": {
                "id": peer_actor_id,
                "username": peer_username,
                "domain": peer_domain,
                "displayName": peer_display_name,
                "actorType": "bsky",
                "avatarUrl": peer_avatar_url,
            },
            "attachments": [],
            "visibility": "direct",
        });
        let mut recipients: HashSet<i64> = HashSet::new();
        recipients.insert(local_actor_id);
        stream_hub.publish_note(recipients, &note_json);
    }

    Ok(())
}

/// 自分が送信したメッセージ（スキップ対象）のカーソルのみを進める。
async fn persist_cursor(pool: &PgPool, thread_root: i64, convo_id: &str, msg_id: &str) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO bsky_convo_links (thread_root_post_id, convo_id, last_synced_message_id)
         VALUES ($1, $2, $3)
         ON CONFLICT (thread_root_post_id) DO UPDATE SET last_synced_message_id = EXCLUDED.last_synced_message_id",
    )
    .bind(thread_root)
    .bind(convo_id)
    .bind(msg_id)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

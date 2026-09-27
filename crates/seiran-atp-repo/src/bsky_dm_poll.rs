//! Bsky DM受信ポーリング。
//!
//! `chat.bsky.convo`はJetstreamに乗らない（私信のため公開ファイヤホースに含まれない）ため、
//! ローカルBskyリンク済みユーザーごとに `listConvos`/`getMessages` を定期ポーリングして
//! 新着メッセージを `posts`（visibility=direct）として取り込む。
//! 認証方式は `docs/skill_atp_rust_programming.md` §17 参照（自己署名サービス認証JWT）。
//!
//! 会話ごとの初回同期（`bsky_convo_links`未登録＝`last_synced_message_id`未設定）は
//! `getMessages`をcursorページングして遡れるだけ遡る（既存DID転入で持ち込んだ会話のように、
//! seiranにとって初見だが実際は長い履歴を持つケースに対応するため）。通常のポーリングは
//! 最新1ページのみで新着を拾えば十分なためページングしない。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use seiran_common::atp::sign_service_auth_jwt;
use seiran_common::generate_snowflake_id;
use seiran_common::repository::{DmRepository, PgDmRepository, dm};
use seiran_common::streaming::StreamHub;
use seiran_common::traits::JobQueue;
use sqlx::PgPool;

use crate::firehose::resolve_or_upsert_bsky_actor;

const CHAT_SERVICE_HOST: &str = "https://api.bsky.chat";
const CHAT_SERVICE_AUD: &str = "did:web:api.bsky.chat";
const POLL_INTERVAL: Duration = Duration::from_secs(60);
/// 初回同期（ページング遡及）1会話あたりの最大取得ページ数。`limit=100`と合わせて
/// 最大2000件相当。異常に長い会話やAPI応答の不具合で無限ループしないための安全弁。
const MAX_INITIAL_SYNC_PAGES: u32 = 20;
/// 初回同期1会話あたりの最大取得メッセージ件数（安全弁、`MAX_INITIAL_SYNC_PAGES`と併用）。
const MAX_INITIAL_SYNC_MESSAGES: usize = 2000;

/// DM受信ポーリングを常駐実行する。
pub async fn run(
    pool: PgPool,
    job_queue: Arc<dyn JobQueue>,
    http: Arc<reqwest::Client>,
    stream_hub: Arc<StreamHub>,
) {
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    loop {
        interval.tick().await;
        if let Err(e) = poll_once(&pool, &job_queue, &http, &stream_hub).await {
            tracing::error!("[BskyDmPoll] ポーリング失敗: {}", e);
        }
    }
}

async fn poll_once(
    pool: &PgPool,
    job_queue: &Arc<dyn JobQueue>,
    http: &reqwest::Client,
    stream_hub: &StreamHub,
) -> Result<(), String> {
    let users = dm::bsky_dm_poll_accounts(pool)
        .await
        .map_err(|e| e.to_string())?;

    for (actor_id, did, pem) in users {
        let account = LocalChatAccount {
            actor_id,
            did: &did,
            pem: &pem,
        };
        if let Err(e) = poll_user(pool, job_queue, http, stream_hub, &account).await {
            // 401は主にDIDがPLCディレクトリ上で無効（テスト用アカウント等）な場合に発生する
            // 想定内のケースのため warn 止まりとし、エラー監視のノイズにしない。
            tracing::warn!("[BskyDmPoll] actor_id={} のポーリング失敗: {}", actor_id, e);
        }
    }
    Ok(())
}

/// DMポーリング対象のローカルアカウント（chat サービスへの認証に使う DID と署名鍵）。
struct LocalChatAccount<'a> {
    actor_id: i64,
    did: &'a str,
    /// サービス間認証JWTの署名鍵（PEM）。
    pem: &'a str,
}

async fn poll_user(
    pool: &PgPool,
    job_queue: &Arc<dyn JobQueue>,
    http: &reqwest::Client,
    stream_hub: &StreamHub,
    account: &LocalChatAccount<'_>,
) -> Result<(), String> {
    let LocalChatAccount { actor_id, did, pem } = *account;
    let jwt = sign_service_auth_jwt(pem, did, CHAT_SERVICE_AUD, "chat.bsky.convo.listConvos")
        .map_err(|e| e.to_string())?;
    let resp = http
        .get(format!(
            "{}/xrpc/chat.bsky.convo.listConvos",
            CHAT_SERVICE_HOST
        ))
        .bearer_auth(&jwt)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("listConvos失敗 status={}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let convos = body
        .get("convos")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for convo in &convos {
        if let Err(e) = sync_convo(pool, job_queue, http, stream_hub, account, convo).await {
            tracing::error!("[BskyDmPoll] convo同期失敗 actor_id={}: {}", actor_id, e);
        }
    }
    Ok(())
}

async fn sync_convo(
    pool: &PgPool,
    job_queue: &Arc<dyn JobQueue>,
    http: &reqwest::Client,
    stream_hub: &StreamHub,
    account: &LocalChatAccount<'_>,
    convo: &serde_json::Value,
) -> Result<(), String> {
    let convo_id = convo
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("convo.idが無い")?;
    let Some(peer_did) = direct_convo_peer_did(convo, account.did) else {
        return Ok(());
    };
    let (mut current_thread_root, last_synced) = load_convo_link(pool, convo_id).await?;
    let (mut new_messages, all_fetched_messages) =
        fetch_convo_messages(http, account, convo_id, last_synced).await?;

    // 相手が付け外ししたリアクションは、新着メッセージを増やさないため新着検知だけでは
    // 拾えない。取得できた全メッセージ（ページング分すべて、新着かどうか問わず）について
    // `reactions`フィールドをDBの記録（`dm_bsky_reactions`）と同期する。`peer_actor_id`は
    // この後の新規メッセージ取り込みでも使うため、新着有無によらず一度だけ解決する
    // （重複upsertを避ける）。
    let peer_actor_id = resolve_or_upsert_bsky_actor(pool, job_queue, http, &peer_did).await?;
    if !all_fetched_messages.is_empty()
        && let Err(e) = sync_message_reactions(
            pool,
            stream_hub,
            account.actor_id,
            account.did,
            peer_actor_id,
            &peer_did,
            &all_fetched_messages,
        )
        .await
    {
        tracing::warn!(
            "[BskyDmPoll] リアクション同期失敗 convo_id={}: {}",
            convo_id,
            e
        );
    }

    new_messages.reverse(); // 古い順に処理する
    for m in &new_messages {
        let msg_id = m
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let sender_did = m
            .get("sender")
            .and_then(|s| s.get("did"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if sender_did == account.did {
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
        let received = ReceivedMessage {
            msg_id: &msg_id,
            text: m.get("text").and_then(|v| v.as_str()).unwrap_or_default(),
            sent_at: m
                .get("sentAt")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(chrono::Utc::now),
        };
        let (post_id, thread_root) = import_peer_message(
            pool,
            account.actor_id,
            convo_id,
            peer_actor_id,
            &received,
            current_thread_root,
        )
        .await?;
        current_thread_root = Some(thread_root);
        publish_received_dm(
            pool,
            stream_hub,
            account.actor_id,
            peer_actor_id,
            post_id,
            &received,
        )
        .await?;
    }
    Ok(())
}

/// 1:1 の会話（`directConvo`）なら相手の DID を返す（グループ会話は対象外）。
fn direct_convo_peer_did(convo: &serde_json::Value, local_did: &str) -> Option<String> {
    let kind = convo
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("directConvo");
    if kind != "directConvo" {
        return None;
    }
    convo
        .get("members")
        .and_then(|v| v.as_array())?
        .iter()
        .filter_map(|m| m.get("did").and_then(|v| v.as_str()))
        .find(|d| *d != local_did)
        .map(str::to_string)
}

/// 会話の同期状態（スレッド起点の post_id, 最後に同期したメッセージID）を読む。
async fn load_convo_link(
    pool: &PgPool,
    convo_id: &str,
) -> Result<(Option<i64>, Option<String>), String> {
    let existing = dm::convo_link(pool, convo_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(match existing {
        Some((root, last)) => (Some(root), last),
        None => (None, None),
    })
}

/// 会話のメッセージを取得し、（前回同期以降の新着, 取得した全メッセージ）を返す（新しい順）。
/// この会話を一度も同期したことが無い（last_synced_message_id未設定）場合のみ、転入で持ち込んだ
/// 会話のような「seiranにとって初見だが実際は長い履歴を持つ」ケースに対応するため、cursor
/// ページングで遡れるだけ遡る（既存DID転入#account_migrationで新たに顕在化した要件）。通常の
/// ポーリング（既に同期済みの会話）は最新1ページのみを見れば新着が拾えるため、ページングしない。
/// 全メッセージはリアクション同期に使う（相手が既存メッセージへリアクションを付け外ししても
/// 新着メッセージは増えないため、新着だけでは検知できない）。
async fn fetch_convo_messages(
    http: &reqwest::Client,
    account: &LocalChatAccount<'_>,
    convo_id: &str,
    last_synced: Option<String>,
) -> Result<(Vec<serde_json::Value>, Vec<serde_json::Value>), String> {
    let LocalChatAccount {
        did: local_did,
        pem: local_pem,
        ..
    } = *account;
    let is_initial_sync = last_synced.is_none();
    let mut new_messages: Vec<serde_json::Value> = Vec::new();
    let mut all_fetched_messages: Vec<serde_json::Value> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut fetched_pages = 0u32;
    loop {
        let jwt = sign_service_auth_jwt(
            local_pem,
            local_did,
            CHAT_SERVICE_AUD,
            "chat.bsky.convo.getMessages",
        )
        .map_err(|e| e.to_string())?;
        let mut url = format!(
            "{}/xrpc/chat.bsky.convo.getMessages?convoId={}&limit=100",
            CHAT_SERVICE_HOST, convo_id
        );
        if let Some(c) = &cursor {
            url.push_str(&format!("&cursor={}", urlencoding::encode(c)));
        }
        let resp = http
            .get(&url)
            .bearer_auth(&jwt)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("getMessages失敗 status={}", resp.status()));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let messages = body
            .get("messages")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        fetched_pages += 1;
        all_fetched_messages.extend(messages.iter().cloned());

        // getMessagesは新しい順で返る。前回同期済みのメッセージIDに到達したら
        // （＝そこから先は既に取り込み済み）そこで打ち切る。
        let mut reached_last_synced = false;
        for m in &messages {
            let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if Some(id.to_string()) == last_synced {
                reached_last_synced = true;
                break;
            }
            new_messages.push(m.clone());
        }

        let next_cursor = body
            .get("cursor")
            .and_then(|v| v.as_str())
            .filter(|c| !c.is_empty())
            .map(|s| s.to_string());
        let should_continue = is_initial_sync
            && !reached_last_synced
            && next_cursor.is_some()
            && !messages.is_empty()
            && fetched_pages < MAX_INITIAL_SYNC_PAGES
            && new_messages.len() < MAX_INITIAL_SYNC_MESSAGES;
        if !should_continue {
            break;
        }
        cursor = next_cursor;
    }
    Ok((new_messages, all_fetched_messages))
}

/// 相手から受信した DM メッセージ1件。
struct ReceivedMessage<'a> {
    msg_id: &'a str,
    text: &'a str,
    sent_at: chrono::DateTime<chrono::Utc>,
}

/// 受信メッセージ1件を取り込み、（post_id, スレッド起点）を返す。「posts INSERT + post_recipients
/// INSERT + カーソル前進」を単一トランザクションでコミットする。複数メッセージの取り込み途中で
/// エラーが起きても、既にコミット済みの分のカーソルは進んでいるため、次回ポーリングでの再取り込みは
/// 未コミット分のみに限定される。bsky_message_id のUNIQUE制約（DO NOTHING）が、それでも起こりうる
/// 二重取り込み（同時ポーリング等）に対する保険になる。
async fn import_peer_message(
    pool: &PgPool,
    local_actor_id: i64,
    convo_id: &str,
    peer_actor_id: i64,
    message: &ReceivedMessage<'_>,
    current_thread_root: Option<i64>,
) -> Result<(i64, i64), String> {
    let ReceivedMessage {
        msg_id,
        text,
        sent_at,
    } = *message;
    let candidate_post_id = generate_snowflake_id(sent_at);
    let candidate_thread_root = current_thread_root.unwrap_or(candidate_post_id);

    dm::import_bsky_dm(
        pool,
        &dm::IncomingBskyDm {
            candidate_post_id,
            candidate_thread_root,
            sender_actor_id: peer_actor_id,
            recipient_actor_id: local_actor_id,
            convo_id,
            message_id: msg_id,
            text,
            sent_at,
        },
    )
    .await
    .map_err(|e| format!("DM受信取り込み失敗: {}", e))
}

/// 受信した DM をローカルユーザーの画面へ WebSocket 配信する。
async fn publish_received_dm(
    pool: &PgPool,
    stream_hub: &StreamHub,
    local_actor_id: i64,
    peer_actor_id: i64,
    post_id: i64,
    message: &ReceivedMessage<'_>,
) -> Result<(), String> {
    let peer_row = seiran_common::repository::actor::lite_rows_for_actors(pool, &[peer_actor_id])
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .next();
    let (peer_username, peer_domain, peer_display_name, peer_avatar_url): (
        String,
        String,
        Option<String>,
        Option<String>,
    ) = match peer_row {
        Some(r) => (r.username, r.domain, r.display_name, r.avatar_url),
        None => (String::new(), String::new(), None, None),
    };

    let note_json = serde_json::json!({
        "id": post_id.to_string(),
        "text": message.text,
        "createdAt": message.sent_at.to_rfc3339(),
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
    Ok(())
}

/// 取得済みメッセージ群の`reactions`フィールドを`dm_bsky_reactions`と同期する。
/// メッセージ自身がまだDB未登録（`posts.bsky_message_id`に対応行が無い、通常は
/// 起きないが安全側）の場合はそのメッセージだけスキップする。実際に変化があった
/// メッセージは、ローカルユーザー（Bsky側の相手には配信不要、WS接続を持たないため）
/// へ`noteUpdated`イベントで即時反映する（`docs/protocols.md` 9節）。
async fn sync_message_reactions(
    pool: &PgPool,
    stream_hub: &StreamHub,
    local_actor_id: i64,
    local_did: &str,
    peer_actor_id: i64,
    peer_did: &str,
    messages: &[serde_json::Value],
) -> Result<(), String> {
    let dm_repo = PgDmRepository::new(pool.clone());
    for m in messages {
        let msg_id = m.get("id").and_then(|v| v.as_str()).unwrap_or_default();
        if msg_id.is_empty() {
            continue;
        }
        let Some(reactions_json) = m.get("reactions").and_then(|v| v.as_array()) else {
            continue;
        };

        let post_id = dm::post_id_by_bsky_message_id(pool, msg_id)
            .await
            .map_err(|e| e.to_string())?;
        let Some(post_id) = post_id else {
            continue;
        };

        let reactions: Vec<(i64, String)> = reactions_json
            .iter()
            .filter_map(|r| {
                let value = r.get("value").and_then(|v| v.as_str())?.to_string();
                let sender_did = r
                    .get("sender")
                    .and_then(|s| s.get("did"))
                    .and_then(|v| v.as_str())?;
                let actor_id = if sender_did == local_did {
                    local_actor_id
                } else if sender_did == peer_did {
                    peer_actor_id
                } else {
                    // 1:1会話の参加者以外（あり得ないはずだが安全側）は無視する。
                    return None;
                };
                Some((actor_id, value))
            })
            .collect();

        let changed = dm_repo
            .sync_bsky_reactions(post_id, &reactions, chrono::Utc::now())
            .await
            .map_err(|e| format!("dm_bsky_reactions同期失敗 post_id={}: {}", post_id, e))?;
        if changed {
            notify_dm_bsky_reactions_changed(
                pool,
                stream_hub,
                post_id,
                local_actor_id,
                peer_actor_id,
            )
            .await
            .map_err(|e| format!("リアクション変更通知失敗 post_id={}: {}", post_id, e))?;
        }
    }
    Ok(())
}

/// `dm_bsky_reactions`の現在の集計を`local_actor_id`視点（`reactedByMe`はローカルユーザー
/// 自身がそのcontentを含むかどうか）で組み立て、`noteUpdated`イベントとしてローカル
/// ユーザーのみへ配信する（`MessagesPage`の`registerReaction`が拾う、`NoteCard`と同じ
/// 汎用イベントを再利用）。`reactorActorId`は常に相手（`peer_actor_id`）扱いにする
/// （ローカル自身の変更は`Job::BskyDmReactionAdd`/`Remove`のAPI応答で即時反映済みのため、
/// ここで拾う変化は基本的に相手発。ローカルユーザーが公式Blueskyアプリ経由で操作した
/// 場合のみ「相手発」と誤認するが、その場合も最終的な集計自体は正しく反映される）。
async fn notify_dm_bsky_reactions_changed(
    pool: &PgPool,
    stream_hub: &StreamHub,
    post_id: i64,
    local_actor_id: i64,
    peer_actor_id: i64,
) -> Result<(), String> {
    let rows =
        seiran_common::repository::note_extras::dm_bsky_reaction_counts_for_posts(pool, &[post_id])
            .await
            .map_err(|e| e.to_string())?;

    let reactions_json: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| serde_json::json!({ "emoji": r.content, "count": r.cnt, "emojiUrl": null }))
        .collect();

    stream_hub.publish_event(
        HashSet::from([local_actor_id]),
        "noteUpdated",
        serde_json::json!({
            "postId": post_id.to_string(),
            "reactions": reactions_json,
            "reactorActorId": peer_actor_id.to_string(),
            "reactorEmoji": serde_json::Value::Null,
        }),
    );
    Ok(())
}

/// 自分が送信したメッセージ（スキップ対象）のカーソルのみを進める。
async fn persist_cursor(
    pool: &PgPool,
    thread_root: i64,
    convo_id: &str,
    msg_id: &str,
) -> Result<(), String> {
    dm::advance_convo_cursor(pool, thread_root, convo_id, msg_id)
        .await
        .map_err(|e| e.to_string())
}

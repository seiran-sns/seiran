//! Jetstream（Bluesky公式の軽量フィルタ済みFirehose）WebSocket クライアント
//!
//! `wss://jetstream1.us-east.bsky.network/subscribe` に `wantedCollections` を指定して
//! 接続し、`app.bsky.feed.post`（新規投稿）と `app.bsky.feed.like`（リアクション連携）の
//! create/delete のみを受信する。Jetstream は Relay Firehose を購読して dag-cbor から
//! 既にJSONへデコード済みのレコードを配信するため、CBOR/CAR/CIDの自前デコードは不要。
//!
//! 投稿はイベントに同梱されるレコード本体（text/createdAt）をそのまま保存する
//! （Jetstream はほぼリアルタイムなので、旧実装にあった AppView 再取得＋インデックス
//! 遅延リトライは不要）。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value as JsonValue;
use sqlx::{PgPool, Row};
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::StreamExt;

use seiran_common::atp::fetch_bsky_profile;
use seiran_common::repository::{
    ActorRepository, PostRepository, ReactionRepository,
    PgActorRepository, PgFollowRepository, PgPostRepository, PgReactionRepository,
};
use seiran_common::streaming::broadcast_reaction_update;
use seiran_common::{generate_snowflake_id, StreamHub};

const JETSTREAM_URL: &str =
    "wss://jetstream1.us-east.bsky.network/subscribe?wantedCollections=app.bsky.feed.post&wantedCollections=app.bsky.feed.like";

pub async fn run(pool: PgPool, http: Arc<reqwest::Client>, stream_hub: Arc<StreamHub>) {
    let mut backoff_secs = 2u64;

    loop {
        eprintln!("[Jetstream] 接続中: {}", JETSTREAM_URL);

        match connect_and_process(&pool, &http, &stream_hub).await {
            Ok(()) => {
                eprintln!("[Jetstream] 接続終了（正常）。再接続します。");
                backoff_secs = 2;
            }
            Err(e) => {
                eprintln!("[Jetstream] エラー: {}。{}秒後に再接続します。", e, backoff_secs);
                sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(120);
            }
        }
    }
}

async fn connect_and_process(
    pool: &PgPool,
    http: &Arc<reqwest::Client>,
    stream_hub: &Arc<StreamHub>,
) -> Result<(), String> {
    let (mut ws_stream, _) = connect_async(JETSTREAM_URL)
        .await
        .map_err(|e| format!("WebSocket 接続失敗: {}", e))?;

    eprintln!("[Jetstream] 接続成功。イベント受信中...");

    while let Some(msg) = ws_stream.next().await {
        let msg = msg.map_err(|e| format!("WebSocket 受信エラー: {}", e))?;

        if let Message::Text(text) = msg {
            if let Err(e) = process_message(&text, pool, http, stream_hub).await {
                eprintln!("[Jetstream] メッセージ処理エラー（スキップ）: {}", e);
            }
        }
    }

    Ok(())
}

/// Jetstream の commit イベント（`kind: "commit"`）。`identity`/`account` は無視する。
#[derive(Deserialize)]
struct JetstreamEvent {
    did: String,
    kind: String,
    commit: Option<JetstreamCommit>,
}

#[derive(Deserialize)]
struct JetstreamCommit {
    operation: String, // "create" | "update" | "delete"
    collection: String,
    rkey: String,
    /// create/update のみ存在。デコード済みのレコード本体（レコードの生 JSON）。
    #[serde(default)]
    record: Option<JsonValue>,
    /// create/update のみ存在。
    #[serde(default)]
    cid: Option<String>,
}

async fn process_message(
    text: &str,
    pool: &PgPool,
    http: &Arc<reqwest::Client>,
    stream_hub: &Arc<StreamHub>,
) -> Result<(), String> {
    let event: JetstreamEvent =
        serde_json::from_str(text).map_err(|e| format!("JSON パースエラー: {}", e))?;

    if event.kind != "commit" {
        return Ok(());
    }
    let Some(commit) = event.commit else {
        return Ok(());
    };
    let did = event.did;

    match commit.collection.as_str() {
        "app.bsky.feed.post" => {
            if commit.operation != "create" {
                return Ok(());
            }
            let (Some(record), Some(cid)) = (commit.record, commit.cid) else {
                return Ok(());
            };
            let Some(body_text) = record.get("text").and_then(|v| v.as_str()) else {
                return Ok(());
            };
            let Some(created_at) = record
                .get("createdAt")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
            else {
                return Ok(());
            };

            // この DID のアクターが DB に存在するか確認
            let actor_row = sqlx::query(
                "SELECT id, username, display_name, avatar_url FROM actors WHERE at_did = $1 LIMIT 1",
            )
            .bind(&did)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("DB検索失敗: {}", e))?;

            let Some(actor_row) = actor_row else {
                return Ok(());
            };
            let actor_id: i64 = actor_row.try_get("id").unwrap_or(0);
            let username: String = actor_row.try_get("username").unwrap_or_default();
            let display_name: Option<String> = actor_row.try_get("display_name").unwrap_or(None);
            let avatar_url: Option<String> = actor_row.try_get("avatar_url").unwrap_or(None);

            let at_uri = format!("at://{}/app.bsky.feed.post/{}", did, commit.rkey);

            // 重複チェック
            let already_saved = sqlx::query("SELECT id FROM posts WHERE at_uri = $1 LIMIT 1")
                .bind(&at_uri)
                .fetch_optional(pool)
                .await
                .map_err(|e| format!("重複チェック失敗: {}", e))?
                .is_some();

            if already_saved {
                return Ok(());
            }

            eprintln!("[Jetstream] 新規ポスト検出: {}", at_uri);

            let pool2 = pool.clone();
            let hub2 = Arc::clone(stream_hub);
            let at_uri2 = at_uri.clone();
            let body_text = body_text.to_string();

            tokio::spawn(async move {
                save_bsky_post(
                    &pool2, &hub2, &at_uri2, &cid, &body_text, created_at,
                    actor_id, &username, display_name.as_deref(), avatar_url.as_deref(),
                ).await;
            });
        }

        "app.bsky.feed.like" => {
            match commit.operation.as_str() {
                "create" => {
                    let Some(record) = commit.record else {
                        return Ok(());
                    };
                    let Some(subject_uri) = record
                        .get("subject")
                        .and_then(|s| s.get("uri"))
                        .and_then(|v| v.as_str())
                    else {
                        return Ok(());
                    };
                    let emoji = record.get("emoji").and_then(|v| v.as_str()).map(|s| s.to_string());

                    let at_uri = format!("at://{}/app.bsky.feed.like/{}", did, commit.rkey);
                    let subject_uri = subject_uri.to_string();
                    let pool2 = pool.clone();
                    let http2 = Arc::clone(http);
                    let hub2 = Arc::clone(stream_hub);
                    tokio::spawn(async move {
                        handle_inbound_like_create(&pool2, &http2, &hub2, &did, &at_uri, &subject_uri, emoji.as_deref()).await;
                    });
                }
                "delete" => {
                    let at_uri = format!("at://{}/app.bsky.feed.like/{}", did, commit.rkey);
                    let pool2 = pool.clone();
                    let hub2 = Arc::clone(stream_hub);
                    tokio::spawn(async move {
                        handle_inbound_like_delete(&pool2, &hub2, &at_uri).await;
                    });
                }
                _ => {}
            }
        }

        _ => {}
    }

    Ok(())
}

/// Jetstream イベントから得た投稿本体を DB に保存し、ローカルフォロワーへ配信する。
/// Jetstream はほぼリアルタイムでレコード本体を同梱してくるため、AppView への
/// 再取得・インデックス遅延リトライは不要（旧 Relay Firehose 直結実装にはあった）。
#[allow(clippy::too_many_arguments)]
async fn save_bsky_post(
    pool: &PgPool,
    stream_hub: &StreamHub,
    at_uri: &str,
    at_cid: &str,
    text: &str,
    created_at: chrono::DateTime<chrono::Utc>,
    actor_id: i64,
    username: &str,
    display_name: Option<&str>,
    avatar_url: Option<&str>,
) {
    let post_id = generate_snowflake_id(created_at);

    let result = sqlx::query(
        "INSERT INTO posts (id, actor_id, body, at_uri, at_cid, created_at)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (at_uri) DO NOTHING",
    )
    .bind(post_id)
    .bind(actor_id)
    .bind(text)
    .bind(at_uri)
    .bind(at_cid)
    .bind(created_at)
    .execute(pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() == 0 => {
            eprintln!("[Jetstream] 重複スキップ: {}", at_uri);
        }
        Ok(_) => {
            eprintln!("[Jetstream] 保存完了: {}", at_uri);

            // ローカルフォロワーへ WebSocket 配信
            let follower_rows = sqlx::query(
                "SELECT f.follower_actor_id FROM follows f
                 JOIN actors a ON a.id = f.follower_actor_id
                 WHERE f.target_actor_id = $1 AND f.status = 'accepted'
                   AND a.actor_type = 'local'",
            )
            .bind(actor_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default();

            let recipients: HashSet<i64> = follower_rows
                .iter()
                .filter_map(|r| r.try_get::<i64, _>("follower_actor_id").ok())
                .collect();

            if !recipients.is_empty() {
                let note_json = serde_json::json!({
                    "id": post_id.to_string(),
                    "text": text,
                    "createdAt": created_at.to_rfc3339(),
                    "user": {
                        "id": actor_id,
                        "username": username,
                        "domain": serde_json::Value::Null,
                        "displayName": display_name,
                        "actorType": "bsky",
                        "avatarUrl": avatar_url,
                    },
                    "attachments": [],
                });
                stream_hub.publish_note(recipients, &note_json);
            }
        }
        Err(e) => eprintln!("[Jetstream] DB 保存失敗: {}", e),
    }
}

// ─── リアクション連携（app.bsky.feed.like）────────────────────────────────

/// ATP Like（`app.bsky.feed.like`）の作成を検知した際の処理。
/// `subject_uri` がローカル投稿の `at_uri` と一致する場合のみ `reactions` へ INSERT し、
/// 通知ベル用イベント（著者のみ）とリアルタイム更新（`noteUpdated`、著者+フォロワー）を送出する。
#[allow(clippy::too_many_arguments)]
async fn handle_inbound_like_create(
    pool: &PgPool,
    http: &reqwest::Client,
    stream_hub: &StreamHub,
    did: &str,
    at_uri: &str,
    subject_uri: &str,
    emoji: Option<&str>,
) {
    let posts_repo = PgPostRepository::new(pool.clone());
    let (post_id, post_author_id) = match posts_repo.find_id_and_actor_by_at_uri(subject_uri).await {
        Ok(Some(pair)) => pair,
        Ok(None) => return, // ローカル投稿ではない（あるいは未取り込み）
        Err(e) => {
            eprintln!("[Jetstream/Like] 対象ポスト検索失敗: {}", e);
            return;
        }
    };

    let actor_id = match resolve_or_upsert_bsky_actor(pool, http, did).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("[Jetstream/Like] liker アクター解決失敗: {}", e);
            return;
        }
    };

    // ATP は「1投稿1いいね」が前提（Like レコード自体が unique）なので content は
    // 常に絵文字1個。emoji フィールドが無ければ ❤️（絵文字ピッカーと同じ、VS16付きハート）として扱う。
    let content = emoji.unwrap_or("❤️");
    let reactions_repo = PgReactionRepository::new(pool.clone());
    if let Err(e) = reactions_repo.insert(post_id, actor_id, "like", content, None, Some(at_uri)).await {
        eprintln!("[Jetstream/Like] reactions INSERT 失敗: {}", e);
        return;
    }

    eprintln!("[Jetstream/Like] post {} に {} を記録（did={}）", post_id, content, did);

    // 通知ベル用（#37）: 自作自演（本尊が自分の投稿を Bsky 側からもいいねした等）は通知しない
    if post_author_id != actor_id {
        let actor_repo = PgActorRepository::new(pool.clone());
        if let Ok(Some(liker)) = actor_repo.find_by_id(actor_id).await {
            stream_hub.publish_event(
                HashSet::from([post_author_id]),
                "reaction",
                serde_json::json!({
                    "postId": post_id.to_string(),
                    "emoji": content,
                    "actor": { "username": liker.username, "domain": liker.domain, "displayName": liker.display_name },
                }),
            );
        }
    }

    let follows_repo = PgFollowRepository::new(pool.clone());
    broadcast_reaction_update(
        stream_hub, &follows_repo, &reactions_repo,
        post_id, post_author_id, actor_id, Some(content),
    ).await;
}

/// ATP Like（`app.bsky.feed.like`）の削除（Unlike）を検知した際の処理。
async fn handle_inbound_like_delete(pool: &PgPool, stream_hub: &StreamHub, at_uri: &str) {
    let reactions_repo = PgReactionRepository::new(pool.clone());
    let deleted = match reactions_repo.delete_by_at_uri(at_uri).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[Jetstream/Unlike] reactions DELETE 失敗: {}", e);
            return;
        }
    };
    let Some((post_id, actor_id)) = deleted else {
        return; // 元々知らないリアクションだった（重複 delete イベント等）
    };

    eprintln!("[Jetstream/Unlike] post {} のリアクションを取消（at_uri={}）", post_id, at_uri);

    let posts_repo = PgPostRepository::new(pool.clone());
    let post_author_id = match posts_repo.find_by_id(post_id).await {
        Ok(Some(p)) => p.actor_id,
        _ => return,
    };

    let follows_repo = PgFollowRepository::new(pool.clone());
    broadcast_reaction_update(
        stream_hub, &follows_repo, &reactions_repo,
        post_id, post_author_id, actor_id, None,
    ).await;
}

/// DID からローカル `actors` 行を解決する。無ければ AppView からプロフィールを取得して upsert する
/// （AP 側 `upsert_remote_fedi_actor` の ATP 版）。
async fn resolve_or_upsert_bsky_actor(pool: &PgPool, http: &reqwest::Client, did: &str) -> Result<i64, String> {
    let actor_repo = PgActorRepository::new(pool.clone());
    if let Ok(Some(actor)) = actor_repo.find_by_did(did).await {
        return Ok(actor.id);
    }

    let profile = fetch_bsky_profile(http, did).await?;
    let new_id = generate_snowflake_id(chrono::Utc::now());
    actor_repo
        .upsert_remote_bsky(
            new_id,
            did,
            &profile.handle,
            profile.display_name.as_deref(),
            profile.avatar.as_deref(),
            chrono::Utc::now(),
        )
        .await
        .map_err(|e| format!("upsert_remote_bsky 失敗: {}", e))
}

//! Bluesky Firehose WebSocket クライアント
//!
//! `wss://bsky.network/xrpc/com.atproto.sync.subscribeRepos` に接続し、
//! 新規ポスト作成イベントを受信して DB に保存し、フォロワーへリアルタイム配信する。

use std::collections::HashSet;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use ciborium::value::Value as CborValue;
use sqlx::{PgPool, Row};
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::StreamExt;

use seiran_common::atp::client::fetch_single_bsky_post;
use seiran_common::{generate_snowflake_id, StreamHub};

const FIREHOSE_URL: &str =
    "wss://bsky.network/xrpc/com.atproto.sync.subscribeRepos";

/// Firehose リスナーを起動する。切断時は指数バックオフで再接続する。
pub async fn run(pool: PgPool, http: Arc<reqwest::Client>, stream_hub: Arc<StreamHub>) {
    let mut backoff_secs = 2u64;

    loop {
        eprintln!("[Firehose] 接続中: {}", FIREHOSE_URL);

        match connect_and_process(&pool, &http, &stream_hub).await {
            Ok(()) => {
                eprintln!("[Firehose] 接続終了（正常）。再接続します。");
                backoff_secs = 2;
            }
            Err(e) => {
                eprintln!("[Firehose] エラー: {}。{}秒後に再接続します。", e, backoff_secs);
                sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(120);
            }
        }
    }
}

async fn connect_and_process(
    pool: &PgPool,
    http: &reqwest::Client,
    stream_hub: &StreamHub,
) -> Result<(), String> {
    let (mut ws_stream, _) = connect_async(FIREHOSE_URL)
        .await
        .map_err(|e| format!("WebSocket 接続失敗: {}", e))?;

    eprintln!("[Firehose] 接続成功。イベント受信中...");

    while let Some(msg) = ws_stream.next().await {
        let msg = msg.map_err(|e| format!("WebSocket 受信エラー: {}", e))?;

        if let Message::Binary(bytes) = msg
            && let Err(e) = process_message(&bytes, pool, http, stream_hub).await {
                eprintln!("[Firehose] メッセージ処理エラー（スキップ）: {}", e);
            }
    }

    Ok(())
}

async fn process_message(
    bytes: &[u8],
    pool: &PgPool,
    http: &reqwest::Client,
    stream_hub: &StreamHub,
) -> Result<(), String> {
    let mut cursor = Cursor::new(bytes);

    let header: CborValue = ciborium::from_reader(&mut cursor)
        .map_err(|e| format!("ヘッダー CBOR パースエラー: {}", e))?;

    let op = extract_int_field(&header, "op").unwrap_or(-999);
    if op != 1 {
        return Ok(());
    }

    let body: CborValue = ciborium::from_reader(&mut cursor)
        .map_err(|e| format!("ボディ CBOR パースエラー: {}", e))?;

    let did = match extract_text_field(&body, "did") {
        Some(d) => d,
        None => return Ok(()),
    };

    let ops = extract_ops(&body);
    for (action, path) in ops {
        if action != "create" {
            continue;
        }
        if !path.starts_with("app.bsky.feed.post/") {
            continue;
        }

        // この DID のアクターが DB に存在するか確認
        let actor_row = sqlx::query(
            "SELECT id, username, display_name, avatar_url FROM actors WHERE at_did = $1 LIMIT 1",
        )
        .bind(&did)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("DB検索失敗: {}", e))?;

        let actor_row = match actor_row {
            Some(r) => r,
            None => continue,
        };
        let actor_id: i64 = actor_row.try_get("id").unwrap_or(0);
        let username: String = actor_row.try_get("username").unwrap_or_default();
        let display_name: Option<String> = actor_row.try_get("display_name").unwrap_or(None);
        let avatar_url: Option<String> = actor_row.try_get("avatar_url").unwrap_or(None);

        let at_uri = format!("at://{}/{}", did, path);

        // 重複チェック
        let already_saved = sqlx::query("SELECT id FROM posts WHERE at_uri = $1 LIMIT 1")
            .bind(&at_uri)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("重複チェック失敗: {}", e))?
            .is_some();

        if already_saved {
            continue;
        }

        eprintln!("[Firehose] 新規ポスト検出: {}", at_uri);

        // AppView から AT URI で正確に取得
        let post = match fetch_single_bsky_post(http, &at_uri).await {
            Ok(Some(p)) => p,
            Ok(None) => {
                eprintln!("[Firehose] AppView からポスト取得失敗（見つからず）: {}", at_uri);
                continue;
            }
            Err(e) => {
                eprintln!("[Firehose] AppView 取得エラー: {}", e);
                continue;
            }
        };

        let post_id = generate_snowflake_id(post.created_at);

        sqlx::query(
            "INSERT INTO posts (id, actor_id, body, at_uri, at_cid, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (at_uri) DO NOTHING",
        )
        .bind(post_id)
        .bind(actor_id)
        .bind(&post.text)
        .bind(&post.uri)
        .bind(&post.cid)
        .bind(post.created_at)
        .execute(pool)
        .await
        .map_err(|e| format!("投稿インサート失敗: {}", e))?;

        eprintln!("[Firehose] 保存完了: {}", post.uri);

        // ローカルフォロワーを取得して WebSocket ストリームへ配信
        let follower_rows = sqlx::query(
            "SELECT f.follower_actor_id FROM follows f
             JOIN actors a ON a.id = f.follower_actor_id
             WHERE f.target_actor_id = $1 AND f.status = 'accepted' AND a.actor_type = 'local'",
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
                "text": post.text,
                "createdAt": post.created_at.to_rfc3339(),
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

    Ok(())
}

// ─── CBOR ユーティリティ ──────────────────────────────────────────────────

fn extract_text_field(val: &CborValue, field: &str) -> Option<String> {
    if let CborValue::Map(map) = val {
        for (k, v) in map {
            if let (CborValue::Text(key), CborValue::Text(text)) = (k, v)
                && key == field {
                    return Some(text.clone());
                }
        }
    }
    None
}

fn extract_int_field(val: &CborValue, field: &str) -> Option<i64> {
    if let CborValue::Map(map) = val {
        for (k, v) in map {
            if let CborValue::Text(key) = k
                && key == field {
                    return match v {
                        CborValue::Integer(i) => i64::try_from(*i).ok(),
                        _ => None,
                    };
                }
        }
    }
    None
}

fn extract_ops(body: &CborValue) -> Vec<(String, String)> {
    let mut result = Vec::new();

    let map = match body {
        CborValue::Map(m) => m,
        _ => return result,
    };

    for (k, v) in map {
        if let CborValue::Text(key) = k {
            if key != "ops" {
                continue;
            }
            if let CborValue::Array(ops) = v {
                for op in ops {
                    if let CborValue::Map(op_map) = op {
                        let mut action: Option<String> = None;
                        let mut path: Option<String> = None;
                        for (ok, ov) in op_map {
                            if let (CborValue::Text(ok_str), CborValue::Text(ov_str)) = (ok, ov) {
                                match ok_str.as_str() {
                                    "action" => action = Some(ov_str.clone()),
                                    "path" => path = Some(ov_str.clone()),
                                    _ => {}
                                }
                            }
                        }
                        if let (Some(a), Some(p)) = (action, path) {
                            result.push((a, p));
                        }
                    }
                }
            }
        }
    }

    result
}

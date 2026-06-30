//! Bluesky Firehose WebSocket クライアント
//!
//! `wss://bsky.network/xrpc/com.atproto.sync.subscribeRepos` に接続し、
//! 新規ポスト作成イベントを受信して DB に保存する。
//!
//! # メッセージフォーマット
//! 各 WebSocket バイナリメッセージは 2 つの DAG-CBOR 値を連結したもの:
//! 1. ヘッダー `{"op": 1, "t": "#commit"}`
//! 2. ボディ `{seq, did, ops: [...], blocks: <CAR bytes>, ...}`
//!
//! ブロック（CAR ファイル）の完全パースは行わず、`ops` に記載された AT URI を
//! AppView REST API で取得するベストエフォート方式を採用する。

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use ciborium::value::Value as CborValue;
use sqlx::PgPool;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::StreamExt;

use seiran_common::atp::client::fetch_atp_history;
use seiran_common::generate_snowflake_id;

const FIREHOSE_URL: &str =
    "wss://bsky.network/xrpc/com.atproto.sync.subscribeRepos";

/// Firehose リスナーを起動する。切断時は指数バックオフで再接続する。
pub async fn run(pool: PgPool, http: Arc<reqwest::Client>) {
    let mut backoff_secs = 2u64;

    loop {
        eprintln!("[Firehose] 接続中: {}", FIREHOSE_URL);

        match connect_and_process(&pool, &http).await {
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

async fn connect_and_process(pool: &PgPool, http: &reqwest::Client) -> Result<(), String> {
    let (mut ws_stream, _) = connect_async(FIREHOSE_URL)
        .await
        .map_err(|e| format!("WebSocket 接続失敗: {}", e))?;

    eprintln!("[Firehose] 接続成功。イベント受信中...");

    while let Some(msg) = ws_stream.next().await {
        let msg = msg.map_err(|e| format!("WebSocket 受信エラー: {}", e))?;

        if let Message::Binary(bytes) = msg {
            if let Err(e) = process_message(&bytes, pool, http).await {
                // 個別イベントのエラーは無視して続行
                eprintln!("[Firehose] メッセージ処理エラー（スキップ）: {}", e);
            }
        }
    }

    Ok(())
}

async fn process_message(bytes: &[u8], pool: &PgPool, http: &reqwest::Client) -> Result<(), String> {
    let mut cursor = Cursor::new(bytes);

    // ヘッダー CBOR を読む
    let header: CborValue = ciborium::from_reader(&mut cursor)
        .map_err(|e| format!("ヘッダー CBOR パースエラー: {}", e))?;

    let op = extract_int_field(&header, "op").unwrap_or(-999);
    if op != 1 {
        // op=1 のみコミット。-1 はエラー、2 はハンドル変更など。
        return Ok(());
    }

    // ボディ CBOR を読む
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
        let exists = sqlx::query("SELECT id FROM actors WHERE at_did = $1 LIMIT 1")
            .bind(&did)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("DB検索失敗: {}", e))?
            .is_some();

        if !exists {
            continue;
        }

        let at_uri = format!("at://{}/{}", did, path);
        eprintln!("[Firehose] 新規ポスト検出: {}", at_uri);

        // AT URI が既存でない場合のみ AppView から取得して保存
        let already_saved = sqlx::query("SELECT id FROM posts WHERE at_uri = $1 LIMIT 1")
            .bind(&at_uri)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("重複チェック失敗: {}", e))?
            .is_some();

        if already_saved {
            continue;
        }

        // AppView REST で単一ポストを取得（最新1件取得の簡易実装）
        // 本来は getPostThread だが、getAuthorFeed+limit=1 で代替
        if let Ok(posts) = fetch_atp_history(http, &did, 1, 1).await {
            for post in posts {
                if post.uri == at_uri {
                    save_atp_post(pool, &did, &post).await?;
                    break;
                }
            }
        }
    }

    Ok(())
}

async fn save_atp_post(
    pool: &PgPool,
    at_did: &str,
    post: &seiran_common::atp::client::BskyPost,
) -> Result<(), String> {
    use sqlx::Row;

    let actor_row = sqlx::query("SELECT id FROM actors WHERE at_did = $1 LIMIT 1")
        .bind(at_did)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("アクターDB検索失敗: {}", e))?;

    let actor_id: i64 = match actor_row {
        Some(row) => row.try_get("id").map_err(|e| format!("id取得失敗: {}", e))?,
        None => return Ok(()),
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
    Ok(())
}

// ─── CBOR ユーティリティ ──────────────────────────────────────────────────

fn extract_text_field(val: &CborValue, field: &str) -> Option<String> {
    if let CborValue::Map(map) = val {
        for (k, v) in map {
            if let (CborValue::Text(key), CborValue::Text(text)) = (k, v) {
                if key == field {
                    return Some(text.clone());
                }
            }
        }
    }
    None
}

fn extract_int_field(val: &CborValue, field: &str) -> Option<i64> {
    if let CborValue::Map(map) = val {
        for (k, v) in map {
            if let CborValue::Text(key) = k {
                if key == field {
                    return match v {
                        CborValue::Integer(i) => i64::try_from(*i).ok(),
                        _ => None,
                    };
                }
            }
        }
    }
    None
}

/// ops 配列から (action, path) ペアを抽出する
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

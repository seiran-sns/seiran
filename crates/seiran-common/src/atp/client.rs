//! AT Protocol AppView クライアント & PDS コミットモジュール
//!
//! - 公開 AppView (`public.api.bsky.app`) から過去ログを取得する（認証不要）
//! - PDS への createSession + createRecord でポストを送信する（要 App Password）

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

const APPVIEW_URL: &str = "https://public.api.bsky.app";

// ─── 型定義 ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BskyPost {
    /// `at://did:plc:.../app.bsky.feed.post/{rkey}`
    pub uri: String,
    pub cid: String,
    pub author_did: String,
    pub author_handle: String,
    pub text: String,
    pub created_at: DateTime<Utc>,
    pub indexed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct AtpSession {
    pub did: String,
    pub handle: String,
    pub access_jwt: String,
    pub refresh_jwt: String,
}

// ─── AppView レスポンス内部型 ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GetAuthorFeedResp {
    feed: Vec<FeedViewPost>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeedViewPost {
    post: PostView,
    /// リポストの場合 `$type` が入る。通常投稿は null。
    reason: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostView {
    uri: String,
    cid: String,
    author: ProfileViewBasic,
    record: serde_json::Value,
    indexed_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileViewBasic {
    did: String,
    handle: String,
}

// ─── PDS セッション/レスポンス内部型 ──────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionResp {
    did: String,
    handle: String,
    access_jwt: String,
    refresh_jwt: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateRecordReq<'a> {
    repo: &'a str,
    collection: &'a str,
    record: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct CreateRecordResp {
    uri: String,
    cid: String,
}

// ─── 公開 API ─────────────────────────────────────────────────────────────

/// Bluesky AppView から過去ログを最大 `max_posts` 件 / `max_days` 日分取得する。
///
/// 公開エンドポイントを使用するため認証不要。
/// `did` は `did:plc:...` 形式のほかハンドル (`user.bsky.social`) も受け付ける。
pub async fn fetch_atp_history(
    client: &reqwest::Client,
    did: &str,
    max_posts: usize,
    max_days: i64,
) -> Result<Vec<BskyPost>, String> {
    let cutoff = Utc::now() - Duration::days(max_days);
    let mut posts: Vec<BskyPost> = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        if posts.len() >= max_posts {
            break;
        }

        let mut url = format!(
            "{}/xrpc/app.bsky.feed.getAuthorFeed?actor={}&limit=100",
            APPVIEW_URL,
            urlencoding::encode(did)
        );
        if let Some(ref c) = cursor {
            url.push_str(&format!("&cursor={}", urlencoding::encode(c)));
        }

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("getAuthorFeed HTTP エラー: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!(
                "getAuthorFeed 失敗 ({}): did={}",
                resp.status(),
                did
            ));
        }

        let body: GetAuthorFeedResp = resp
            .json()
            .await
            .map_err(|e| format!("getAuthorFeed パースエラー: {}", e))?;

        let next_cursor = body.cursor.clone();
        let mut reached_cutoff = false;

        for item in body.feed {
            // リポストは除外
            if item.reason.is_some() {
                continue;
            }

            let post = item.post;
            let record = &post.record;

            // `app.bsky.feed.post` のみ対象
            if record.get("$type").and_then(|v| v.as_str()) != Some("app.bsky.feed.post") {
                continue;
            }

            let text = record
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let created_at = record
                .get("createdAt")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                .unwrap_or_else(Utc::now);

            if created_at < cutoff {
                reached_cutoff = true;
                break;
            }

            let indexed_at = post
                .indexed_at
                .parse::<DateTime<Utc>>()
                .unwrap_or_else(|_| Utc::now());

            posts.push(BskyPost {
                uri: post.uri,
                cid: post.cid,
                author_did: post.author.did,
                author_handle: post.author.handle,
                text,
                created_at,
                indexed_at,
            });

            if posts.len() >= max_posts {
                break;
            }
        }

        if reached_cutoff || next_cursor.is_none() || posts.len() >= max_posts {
            break;
        }
        cursor = next_cursor;
    }

    Ok(posts)
}

/// AppView `app.bsky.feed.getPosts` で AT URI を指定して単一ポストを取得する。
///
/// firehose から通知された AT URI を正確に取得するための用途。
pub async fn fetch_single_bsky_post(
    client: &reqwest::Client,
    at_uri: &str,
) -> Result<Option<BskyPost>, String> {
    let url = format!(
        "{}/xrpc/app.bsky.feed.getPosts?uris={}",
        APPVIEW_URL,
        urlencoding::encode(at_uri)
    );

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("getPosts HTTP エラー: {}", e))?;

    if !resp.status().is_success() {
        return Ok(None);
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("getPosts パースエラー: {}", e))?;

    let posts = match json["posts"].as_array() {
        Some(a) if !a.is_empty() => a,
        _ => return Ok(None),
    };

    let p = &posts[0];
    let text = p["record"]["text"].as_str().unwrap_or("").to_string();
    let created_at_str = p["record"]["createdAt"].as_str().unwrap_or("");
    let created_at = created_at_str
        .parse::<DateTime<Utc>>()
        .unwrap_or_else(|_| Utc::now());

    Ok(Some(BskyPost {
        uri: p["uri"].as_str().unwrap_or("").to_string(),
        cid: p["cid"].as_str().unwrap_or("").to_string(),
        author_did: p["author"]["did"].as_str().unwrap_or("").to_string(),
        author_handle: p["author"]["handle"].as_str().unwrap_or("").to_string(),
        text,
        created_at,
        indexed_at: Utc::now(),
    }))
}

/// PDS に対して `com.atproto.server.createSession` を呼び出し、セッションを取得する。
///
/// `identifier` はハンドルまたは DID。`password` は App Password を推奨。
pub async fn create_atp_session(
    client: &reqwest::Client,
    pds_endpoint: &str,
    identifier: &str,
    password: &str,
) -> Result<AtpSession, String> {
    let resp = client
        .post(format!(
            "{}/xrpc/com.atproto.server.createSession",
            pds_endpoint
        ))
        .json(&serde_json::json!({
            "identifier": identifier,
            "password": password,
        }))
        .send()
        .await
        .map_err(|e| format!("createSession HTTP エラー: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("createSession 失敗 ({}): {}", status, body));
    }

    let session: CreateSessionResp = resp
        .json()
        .await
        .map_err(|e| format!("createSession パースエラー: {}", e))?;

    Ok(AtpSession {
        did: session.did,
        handle: session.handle,
        access_jwt: session.access_jwt,
        refresh_jwt: session.refresh_jwt,
    })
}

/// PDS に `app.bsky.feed.post` レコードを作成する。
///
/// 成功時は `(at_uri, cid)` を返す。
pub async fn create_atp_post(
    client: &reqwest::Client,
    session: &AtpSession,
    pds_endpoint: &str,
    text: &str,
    created_at: DateTime<Utc>,
) -> Result<(String, String), String> {
    let record = serde_json::json!({
        "$type": "app.bsky.feed.post",
        "text": text,
        "createdAt": created_at.to_rfc3339(),
        "langs": ["ja"],
    });

    let req_body = CreateRecordReq {
        repo: &session.did,
        collection: "app.bsky.feed.post",
        record,
    };

    let resp = client
        .post(format!(
            "{}/xrpc/com.atproto.repo.createRecord",
            pds_endpoint
        ))
        .bearer_auth(&session.access_jwt)
        .json(&req_body)
        .send()
        .await
        .map_err(|e| format!("createRecord HTTP エラー: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("createRecord 失敗 ({}): {}", status, body));
    }

    let result: CreateRecordResp = resp
        .json()
        .await
        .map_err(|e| format!("createRecord パースエラー: {}", e))?;

    Ok((result.uri, result.cid))
}

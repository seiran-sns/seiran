//! AT Protocol AppView クライアント & PDS コミットモジュール
//!
//! - 公開 AppView (`public.api.bsky.app`) から過去ログを取得する（認証不要）
//! - PDS への createSession + createRecord でポストを送信する（要 App Password）

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// Bsky AppView のベース URL。未設定時は本番の公開AppView。
/// E2E テストではローカルのスタブサーバーに向けるために使う。
fn appview_base_url() -> String {
    std::env::var("ATP_APPVIEW_URL")
        .unwrap_or_else(|_| "https://public.api.bsky.app".to_string())
        .trim_end_matches('/')
        .to_string()
}

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
            appview_base_url(),
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
        appview_base_url(),
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

/// `app.bsky.actor.profile` の `pinnedPost`（`com.atproto.repo.strongRef`）。
#[derive(Debug, Clone, Deserialize)]
pub struct BskyPinnedPostRef {
    pub uri: String,
    pub cid: String,
}

/// `app.bsky.actor.getProfile` レスポンスから必要なフィールドのみ取り出したもの。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BskyProfile {
    pub did: String,
    pub handle: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub avatar: Option<String>,
    /// ピン留め投稿（#61）。Bsky はピン留めを1件までしかサポートしない。
    #[serde(default)]
    pub pinned_post: Option<BskyPinnedPostRef>,
}

/// Bsky ポストを `posts` テーブルへ反映し、ローカル post_id を返す（既存なら既存の id、
/// 無ければ新規挿入）。リモートアクターのピン留め（`pinnedPost`）同期専用（#61）。
pub async fn upsert_bsky_post(
    pool: &sqlx::PgPool,
    actor_id: i64,
    post: &BskyPost,
) -> Result<i64, sqlx::Error> {
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT id FROM posts WHERE at_uri = $1 LIMIT 1")
        .bind(&post.uri)
        .fetch_optional(pool)
        .await?
    {
        return Ok(id);
    }

    let post_id = crate::generate_snowflake_id(post.created_at);
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
    .await?;

    // ON CONFLICT で INSERT がスキップされた場合（並行同期の競合）に備え、確定した id を引き直す。
    sqlx::query_scalar::<_, i64>("SELECT id FROM posts WHERE at_uri = $1 LIMIT 1")
        .bind(&post.uri)
        .fetch_one(pool)
        .await
}

/// AppView `app.bsky.actor.getProfile` でプロフィールを取得する。
///
/// `actor` はハンドル（`alice.bsky.social`）または DID（`did:plc:...`）。
/// フォロー処理（アクター登録）とプロフィール表示の両方から使う共通のエントリポイント。
pub async fn fetch_bsky_profile(client: &reqwest::Client, actor: &str) -> Result<BskyProfile, String> {
    let url = format!(
        "{}/xrpc/app.bsky.actor.getProfile?actor={}",
        appview_base_url(),
        urlencoding::encode(actor)
    );

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("getProfile HTTP エラー: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("getProfile 失敗 ({}): actor={}", resp.status(), actor));
    }

    resp.json::<BskyProfile>()
        .await
        .map_err(|e| format!("getProfile パースエラー: {}", e))
}

/// AppView `app.bsky.feed.searchPosts` でポストを全文検索する。
///
/// 戻り値: (at_uri リスト, 次ページカーソル)。エラー時は空リストを返す（呼び出し元は
/// ローカル DB 検索結果のみへフォールバックする設計のため、エラーを致命扱いしない）。
pub async fn search_appview_posts(
    client: &reqwest::Client,
    query: &str,
    cursor: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut url = format!(
        "{}/xrpc/app.bsky.feed.searchPosts?q={}&limit=25",
        appview_base_url(),
        urlencoding::encode(query)
    );
    if let Some(c) = cursor {
        url.push_str(&format!("&cursor={}", urlencoding::encode(c)));
    }

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[atp::search_appview_posts] AppView フェッチ失敗: {}", e);
            return (vec![], None);
        }
    };

    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("[atp::search_appview_posts] AppView JSON パース失敗: {}", e);
            return (vec![], None);
        }
    };

    let cursor_next = json["cursor"].as_str().map(str::to_string);
    let uris: Vec<String> = json["posts"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p["uri"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    (uris, cursor_next)
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

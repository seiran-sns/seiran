//! AT Protocol AppView クライアント & PDS コミットモジュール
//!
//! - 公開 AppView (`api.bsky.app`) から過去ログを取得する（認証不要）
//! - PDS への createSession + createRecord でポストを送信する（要 App Password）

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::queue::worker::priority;
use crate::traits::{Job, JobQueue};

/// Bsky AppView のベース URL。未設定時は本番の公開AppView。
/// E2E テストではローカルのスタブサーバーに向けるために使う。
fn appview_base_url() -> String {
    std::env::var("ATP_APPVIEW_URL")
        .unwrap_or_else(|_| "https://api.bsky.app".to_string())
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
    pub author_display_name: Option<String>,
    pub author_avatar: Option<String>,
    pub text: String,
    pub created_at: DateTime<Utc>,
    pub indexed_at: DateTime<Utc>,
    /// `record.embed`（画像・動画・URLカード・引用）。存在すればそのまま保持し、
    /// `upsert_bsky_post` で `parse_bsky_embed_attachments` 等により添付を復元する。
    pub embed: Option<serde_json::Value>,
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
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    avatar: Option<String>,
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

            let embed = record.get("embed").cloned();

            posts.push(BskyPost {
                uri: post.uri,
                cid: post.cid,
                author_did: post.author.did,
                author_handle: post.author.handle,
                author_display_name: post.author.display_name,
                author_avatar: post.author.avatar,
                text,
                created_at,
                indexed_at,
                embed,
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
    let embed = p["record"]["embed"]
        .as_object()
        .map(|_| p["record"]["embed"].clone());

    Ok(Some(BskyPost {
        uri: p["uri"].as_str().unwrap_or("").to_string(),
        cid: p["cid"].as_str().unwrap_or("").to_string(),
        author_did: p["author"]["did"].as_str().unwrap_or("").to_string(),
        author_handle: p["author"]["handle"].as_str().unwrap_or("").to_string(),
        author_display_name: p["author"]["displayName"].as_str().map(str::to_string),
        author_avatar: p["author"]["avatar"].as_str().map(str::to_string),
        text,
        created_at,
        indexed_at: Utc::now(),
        embed,
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
    queue: &Arc<dyn JobQueue>,
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
    let result = sqlx::query(
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
    let final_id = sqlx::query_scalar::<_, i64>("SELECT id FROM posts WHERE at_uri = $1 LIMIT 1")
        .bind(&post.uri)
        .fetch_one(pool)
        .await?;

    // 実際にこのリクエストで新規作成できた場合のみ添付・URLカードを復元する。
    // ON CONFLICT でスキップされた場合（並行競合）は、先に作成した側で処理済みのはず。
    if result.rows_affected() > 0 {
        if let Some(embed) = &post.embed {
            let attachments = crate::atp::parse_bsky_embed_attachments(embed, &post.author_did);
            for (position, att) in attachments.iter().enumerate() {
                if let Err(e) = sqlx::query(
                    "INSERT INTO post_attachments (post_id, media_file_id, remote_url, remote_mime_type, remote_thumbnail_url, is_sensitive, is_gif, position)
                     VALUES ($1, NULL, $2, $3, $4, false, $5, $6)
                     ON CONFLICT (post_id, position) DO NOTHING",
                )
                .bind(final_id)
                .bind(&att.url)
                .bind(&att.mime_type)
                .bind(att.thumbnail_url.as_deref())
                .bind(att.is_gif)
                .bind(position as i16)
                .execute(pool)
                .await
                {
                    tracing::error!("[upsert_bsky_post] 添付URL保存失敗（スキップ）: {}", e);
                }
            }

            if let Some(card) = crate::atp::parse_bsky_embed_link_card(embed, &post.author_did) {
                let insert_result = sqlx::query(
                    "INSERT INTO post_link_cards (post_id, position, url, title, description, thumbnail_url)
                     VALUES ($1, 0, $2, $3, $4, $5)",
                )
                .bind(final_id)
                .bind(&card.url)
                .bind(&card.title)
                .bind(&card.description)
                .bind(card.thumbnail_url.as_deref())
                .execute(pool)
                .await;
                match insert_result {
                    Ok(_) => {
                        if let Err(e) = queue
                            .enqueue(
                                Job::LinkCardEmbedResolve {
                                    post_id: final_id,
                                    position: 0,
                                    url: card.url.clone(),
                                },
                                priority::LOW,
                            )
                            .await
                        {
                            tracing::error!(
                                "[upsert_bsky_post] LinkCardEmbedResolve enqueue失敗: {}",
                                e
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!("[upsert_bsky_post] post_link_cards 保存失敗（スキップ）: {}", e);
                    }
                }
            }
        }
    }

    Ok(final_id)
}

/// AppView `app.bsky.actor.getProfile` でプロフィールを取得する。
///
/// `actor` はハンドル（`alice.bsky.social`）または DID（`did:plc:...`）。
/// フォロー処理（アクター登録）とプロフィール表示の両方から使う共通のエントリポイント。
pub async fn fetch_bsky_profile(
    client: &reqwest::Client,
    actor: &str,
) -> Result<BskyProfile, String> {
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
        return Err(format!(
            "getProfile 失敗 ({}): actor={}",
            resp.status(),
            actor
        ));
    }

    resp.json::<BskyProfile>()
        .await
        .map_err(|e| format!("getProfile パースエラー: {}", e))
}

/// `app.bsky.graph.getFollowers` レスポンスの `followers` 配列の1要素。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BskyFollowerProfile {
    pub did: String,
    pub handle: String,
    pub display_name: Option<String>,
    pub avatar: Option<String>,
}

/// AppView `app.bsky.graph.getFollowers` でフォロワー一覧をページングして取得する。
///
/// 認証不要の公開エンドポイント。`bsky_follower_poll` がローカルユーザーごとに定期ポーリング
/// する用途（Jetstream の `wantedDids` はフォロー元を事前検知できないため、この方式で
/// リモート Bsky アクターからのフォローを検知する）。
/// 戻り値: (フォロワー一覧, 次ページカーソル)。
pub async fn fetch_bsky_followers(
    client: &reqwest::Client,
    actor_did: &str,
    cursor: Option<&str>,
    limit: u32,
) -> Result<(Vec<BskyFollowerProfile>, Option<String>), String> {
    let mut url = format!(
        "{}/xrpc/app.bsky.graph.getFollowers?actor={}&limit={}",
        appview_base_url(),
        urlencoding::encode(actor_did),
        limit
    );
    if let Some(c) = cursor {
        url.push_str(&format!("&cursor={}", urlencoding::encode(c)));
    }

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("getFollowers HTTP エラー: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "getFollowers 失敗 ({}): actor={}",
            resp.status(),
            actor_did
        ));
    }

    #[derive(Debug, Deserialize)]
    struct GetFollowersResp {
        followers: Vec<BskyFollowerProfile>,
        cursor: Option<String>,
    }

    let body: GetFollowersResp = resp
        .json()
        .await
        .map_err(|e| format!("getFollowers パースエラー: {}", e))?;

    Ok((body.followers, body.cursor))
}

/// AppView `app.bsky.feed.searchPosts` でポストを全文検索する。
///
/// 戻り値: (post viewの正規化結果, 次ページカーソル)。`limit`と`until`はAppViewへ
/// そのまま渡す。エラー時は空リストを返す（呼び出し元はローカル DB 検索結果のみへ
/// フォールバックする設計のため、エラーを致命扱いしない）。
pub async fn search_appview_posts(
    client: &reqwest::Client,
    query: &str,
    cursor: Option<&str>,
    limit: usize,
    until: Option<DateTime<Utc>>,
) -> (Vec<BskyPost>, Option<String>) {
    let mut url = format!(
        "{}/xrpc/app.bsky.feed.searchPosts?q={}&limit={}",
        appview_base_url(),
        urlencoding::encode(query),
        limit.clamp(1, 100),
    );
    if let Some(c) = cursor {
        url.push_str(&format!("&cursor={}", urlencoding::encode(c)));
    }
    if let Some(until) = until {
        url.push_str(&format!(
            "&until={}",
            urlencoding::encode(&until.to_rfc3339())
        ));
    }

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[atp::search_appview_posts] AppView フェッチ失敗: {}", e);
            return (vec![], None);
        }
    };

    if !resp.status().is_success() {
        tracing::error!(
            "[atp::search_appview_posts] AppView がエラーを返しました: {} ({})",
            resp.status(),
            url
        );
        return (vec![], None);
    }
    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("[atp::search_appview_posts] AppView JSON パース失敗: {}", e);
            return (vec![], None);
        }
    };

    let cursor_next = json["cursor"].as_str().map(str::to_string);
    let posts: Vec<BskyPost> = json["posts"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|p| {
                    let uri = p["uri"].as_str()?.to_string();
                    let cid = p["cid"].as_str()?.to_string();
                    let author_did = p["author"]["did"].as_str()?.to_string();
                    let author_handle = p["author"]["handle"].as_str()?.to_string();
                    let created_at = p["record"]["createdAt"]
                        .as_str()
                        .and_then(|value| value.parse::<DateTime<Utc>>().ok())
                        .unwrap_or_else(Utc::now);
                    let indexed_at = p["indexedAt"]
                        .as_str()
                        .and_then(|value| value.parse::<DateTime<Utc>>().ok())
                        .unwrap_or(created_at);
                    Some(BskyPost {
                        uri,
                        cid,
                        author_did,
                        author_handle,
                        author_display_name: p["author"]["displayName"]
                            .as_str()
                            .map(str::to_string),
                        author_avatar: p["author"]["avatar"].as_str().map(str::to_string),
                        text: p["record"]["text"].as_str().unwrap_or("").to_string(),
                        created_at,
                        indexed_at,
                        embed: p["record"].get("embed").cloned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    (posts, cursor_next)
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

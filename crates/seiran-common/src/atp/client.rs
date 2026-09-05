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
    /// `record.facets`（リンク・メンション）。存在すればそのまま保持し、`apply_bsky_facets`
    /// で本文への焼き込みと mention_facets 抽出に使う。
    pub facets: Option<serde_json::Value>,
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

/// `BskyPost::facets`（AppView `record.facets` の生 JSON）を本文へ焼き込む。
/// facets が無い・パース失敗時は本文をそのまま返す（取得経路自体は失敗させない）。
pub fn apply_bsky_post_facets(
    text: &str,
    facets: Option<&serde_json::Value>,
) -> (String, serde_json::Value) {
    let parsed_facets: Vec<crate::atp::ParsedFacet> = facets
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    crate::atp::apply_bsky_facets(text, parsed_facets)
}

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
            let facets = record.get("facets").cloned();

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
                facets,
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
    let facets = p["record"]["facets"]
        .as_array()
        .map(|_| p["record"]["facets"].clone());

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
        facets,
    }))
}

/// `com.atproto.repo.getRecord` で任意コレクションのレコード`value`だけを取得する
/// （AppView経由、`api.bsky.app`は主要`com.atproto.*`読み取りメソッドの透過プロキシとして
/// 動作する）。レコード不在（404）・取得失敗時は`None`（呼び出し側は「制限なし」として扱う）。
pub async fn get_record_value(
    client: &reqwest::Client,
    repo: &str,
    collection: &str,
    rkey: &str,
) -> Option<serde_json::Value> {
    let url = format!(
        "{}/xrpc/com.atproto.repo.getRecord?repo={}&collection={}&rkey={}",
        appview_base_url(),
        urlencoding::encode(repo),
        urlencoding::encode(collection),
        urlencoding::encode(rkey),
    );
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    json.get("value").cloned()
}

/// Bsky投稿の `at://` URI から末尾の rkey を取り出す。
fn rkey_from_at_uri(at_uri: &str) -> Option<&str> {
    at_uri.rsplit('/').next().filter(|s| !s.is_empty())
}

/// リモートDIDが自己申告する`org.seiran.actor.declaration`（rkey固定`self`）の
/// `apActorUri`を取得する（#236、リモートseiranアクターの相互申告マージ用）。
/// レコード不在・取得失敗時は`None`（このDIDはseiranアクターではない、またはまだ
/// 宣言していない、として扱う）。
pub async fn fetch_seiran_actor_declaration(client: &reqwest::Client, did: &str) -> Option<String> {
    let value = get_record_value(client, did, "org.seiran.actor.declaration", "self").await?;
    value
        .get("apActorUri")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Bsky投稿のthreadgate（返信許可ルール）・postgate（引用可否）を取得する。
///
/// 両方とも投稿と同じ`rkey`を持つ（AT Protocol標準の規約）ため、投稿の`at_uri`から
/// 直接rkeyを取り出して個別に`getRecord`する。
///
/// 戻り値:
/// - `reply_allow`: `None`=制限なし（threadgateレコード無し）。`Some(値)`はthreadgateレコードの
///   `allow`フィールドそのもの（未指定なら空配列として正規化、`[]`は「誰も返信不可」を意味する）。
///   評価（メンション/フォロー/リスト判定）は表示側（`seiran-api`）が`posts.mention_facets`・
///   `follows`・`lists`テーブルを使って行う（ここでは生ルールの取得のみ）。
/// - `quote_disabled`: postgateの`embeddingRules`に`#disableRule`が含まれるか
///   （postgateは仕様上「全員可」「全員不可」の二値のみで部分許可は無い）。
pub async fn fetch_bsky_gates(
    client: &reqwest::Client,
    author_did: &str,
    post_at_uri: &str,
) -> (Option<serde_json::Value>, bool) {
    let Some(rkey) = rkey_from_at_uri(post_at_uri) else {
        return (None, false);
    };

    let (threadgate, postgate) = tokio::join!(
        get_record_value(client, author_did, "app.bsky.feed.threadgate", rkey),
        get_record_value(client, author_did, "app.bsky.feed.postgate", rkey),
    );

    let reply_allow = threadgate.map(|v| {
        v.get("allow")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]))
    });

    let quote_disabled = postgate
        .and_then(|v| v.get("embeddingRules").and_then(|r| r.as_array().cloned()))
        .map(|rules| {
            rules.iter().any(|r| {
                r.get("$type").and_then(|t| t.as_str())
                    == Some("app.bsky.feed.postgate#disableRule")
            })
        })
        .unwrap_or(false);

    (reply_allow, quote_disabled)
}

/// `app.bsky.graph.getList` でリストの全メンバーDIDを取得する（ページング込み）。
/// threadgate の listRule 評価用（`bsky_remote_list_membership_cache`、
/// `Job::BskyListMembershipResolve`）。取得失敗時はこれまでに集められた分のみ返す。
pub async fn fetch_bsky_list_members(client: &reqwest::Client, list_uri: &str) -> Vec<String> {
    let mut dids = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut url = format!(
            "{}/xrpc/app.bsky.graph.getList?list={}&limit=100",
            appview_base_url(),
            urlencoding::encode(list_uri)
        );
        if let Some(c) = &cursor {
            url.push_str(&format!("&cursor={}", urlencoding::encode(c)));
        }

        let resp = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    "[fetch_bsky_list_members] HTTPエラー list={}: {}",
                    list_uri,
                    e
                );
                break;
            }
        };
        if !resp.status().is_success() {
            break;
        }
        let json: serde_json::Value = match resp.json().await {
            Ok(j) => j,
            Err(_) => break,
        };
        let Some(items) = json["items"].as_array() else {
            break;
        };
        for item in items {
            if let Some(did) = item["subject"]["did"].as_str() {
                dids.push(did.to_string());
            }
        }

        cursor = json["cursor"].as_str().map(str::to_string);
        if cursor.is_none() || items.is_empty() {
            break;
        }
        // 想定外の巨大リストで無限ループ・過剰リクエストにならないよう上限を設ける。
        if dids.len() >= 5000 {
            break;
        }
    }
    dids
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
    http: &reqwest::Client,
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
    let (body, mention_facets) = apply_bsky_post_facets(&post.text, post.facets.as_ref());
    let result = sqlx::query(
        "INSERT INTO posts (id, actor_id, body, at_uri, at_cid, created_at, mention_facets)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (at_uri) DO NOTHING",
    )
    .bind(post_id)
    .bind(actor_id)
    .bind(&body)
    .bind(&post.uri)
    .bind(&post.cid)
    .bind(post.created_at)
    .bind(&mention_facets)
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
                        tracing::error!(
                            "[upsert_bsky_post] post_link_cards 保存失敗（スキップ）: {}",
                            e
                        );
                    }
                }
            }
        }

        // 返信許可（threadgate）・引用可否（postgate）を取得して保存する（#返信/引用グレーアウト）。
        // 取得失敗時は両方とも「制限なし」のまま（デフォルト値: bsky_reply_allow=NULL,
        // bsky_quote_disabled=false）にしておくのが安全（誤ってボタンをグレーアウトしない）。
        let (reply_allow, quote_disabled) =
            fetch_bsky_gates(http, &post.author_did, &post.uri).await;
        if let Err(e) = sqlx::query(
            "UPDATE posts SET bsky_reply_allow = $1, bsky_quote_disabled = $2 WHERE id = $3",
        )
        .bind(&reply_allow)
        .bind(quote_disabled)
        .bind(final_id)
        .execute(pool)
        .await
        {
            tracing::error!("[upsert_bsky_post] gate情報保存失敗（スキップ）: {}", e);
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
                        facets: p["record"].get("facets").cloned(),
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

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use seiran_common::ap::{fetch_actor, plain_to_html, sign_and_post, verify_signature};
use seiran_common::queue::create_job_queue;
use seiran_common::traits::{Job, JobQueue};
use seiran_common::{generate_snowflake_id, get_db_pool, SecretsFile};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::sync::Arc;

struct AppState {
    db: PgPool,
    job_queue: Arc<dyn JobQueue>,
    local_domain: String,
    ap_public_key_pem: String,
    ap_private_key_pem: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    let local_domain = std::env::var("LOCAL_DOMAIN").unwrap_or_else(|_| "localhost".to_string());

    let db = get_db_pool().await?;
    let job_queue = create_job_queue();

    let secrets_file = SecretsFile::from_env();
    let secrets = secrets_file.load_or_create()?;
    let ap_public_key_pem = secrets.ap_public_key_pem.unwrap_or_default();
    let ap_private_key_pem = secrets.ap_private_key_pem.unwrap_or_default();

    let state = Arc::new(AppState {
        db,
        job_queue,
        local_domain,
        ap_public_key_pem,
        ap_private_key_pem,
    });

    let app = Router::new()
        .route("/.well-known/webfinger", get(webfinger_handler))
        .route("/.well-known/nodeinfo", get(nodeinfo_discovery_handler))
        .route("/nodeinfo/2.1", get(nodeinfo_handler))
        .route("/inbox", post(inbox_handler))
        .route("/users/:username", get(actor_handler))
        .route("/users/:username/outbox", get(outbox_handler))
        .with_state(state);

    let port = std::env::var("FEDERATION_INBOX_PORT").unwrap_or_else(|_| "3001".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("[seiran-federation-inbox] 起動: http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

// =====================================================================
// POST /inbox — AP アクティビティ受信
// =====================================================================

async fn inbox_handler(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let header_map: HashMap<String, String> = headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|val| (k.as_str().to_lowercase(), val.to_string()))
        })
        .collect();

    let signature = match header_map.get("signature") {
        Some(s) => s.clone(),
        None => {
            return (StatusCode::UNAUTHORIZED, "署名ヘッダーが見つかりません").into_response();
        }
    };

    match verify_signature("POST", "/inbox", &header_map, &signature).await {
        Ok(true) => {}
        Ok(false) => {
            return (StatusCode::UNAUTHORIZED, "署名検証失敗").into_response();
        }
        Err(e) => {
            eprintln!("[Inbox] 署名検証エラー: {}", e);
            return (StatusCode::UNAUTHORIZED, format!("署名エラー: {}", e)).into_response();
        }
    }

    let raw_activity = String::from_utf8_lossy(&body).to_string();
    eprintln!("[Inbox] アクティビティ受信 ({} bytes)", raw_activity.len());

    let activity: serde_json::Value = match serde_json::from_str(&raw_activity) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[Inbox] JSON パースエラー: {}", e);
            return (StatusCode::BAD_REQUEST, "JSON パースエラー").into_response();
        }
    };

    match activity["type"].as_str().unwrap_or("") {
        "Follow" => {
            let state_clone = state.clone();
            let activity_clone = activity.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_follow(activity_clone, state_clone).await {
                    eprintln!("[Inbox/Follow] 処理エラー: {}", e);
                }
            });
        }
        "Create" => {
            if activity["object"]["type"].as_str() == Some("Note") {
                let state_clone = state.clone();
                let activity_clone = activity.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_create_note(activity_clone, state_clone).await {
                        eprintln!("[Inbox/Create] 処理エラー: {}", e);
                    }
                });
            }
        }
        "Accept" => {
            let state_clone = state.clone();
            let activity_clone = activity.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_accept(activity_clone, state_clone).await {
                    eprintln!("[Inbox/Accept] 処理エラー: {}", e);
                }
            });
        }
        "Undo" => {
            let state_clone = state.clone();
            let activity_clone = activity.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_undo(activity_clone, state_clone).await {
                    eprintln!("[Inbox/Undo] 処理エラー: {}", e);
                }
            });
        }
        other => {
            eprintln!("[Inbox] type={} をジョブキューへエンキュー", other);
            if let Err(e) = state
                .job_queue
                .enqueue(Job::InboundActivityProcess { raw_activity }, 10)
                .await
            {
                eprintln!("[Inbox] エンキュー失敗: {}", e);
            }
        }
    }

    (StatusCode::ACCEPTED, "").into_response()
}

// Follow アクティビティを処理し Accept を送信する
async fn handle_follow(
    activity: serde_json::Value,
    state: Arc<AppState>,
) -> Result<(), String> {
    let follower_uri = activity["actor"]
        .as_str()
        .ok_or("Follow: actor フィールドがありません")?;
    let target_uri = activity["object"]
        .as_str()
        .ok_or("Follow: object フィールドがありません")?;

    // target_uri から "https://{domain}/users/{username}" のユーザー名を抽出
    let local_username = target_uri
        .rsplit('/')
        .next()
        .ok_or("Follow: object URI からユーザー名を抽出できません")?;

    // ローカルアクターが実在するか確認
    let local_row = sqlx::query(
        "SELECT id FROM actors WHERE username = $1 AND domain = $2 AND actor_type = 'local' LIMIT 1",
    )
    .bind(local_username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
    .ok_or_else(|| format!("ローカルアクター '{}' が存在しません", local_username))?;

    let local_actor_id: i64 = local_row.try_get("id").map_err(|e| e.to_string())?;

    // リモートアクタードキュメントを取得（inbox URL・display_name 用）
    let remote_ap = fetch_actor(follower_uri).await?;
    let remote_inbox = remote_ap
        .inbox
        .as_deref()
        .ok_or("Follow: リモートアクターの inbox が取得できません")?
        .to_string();

    let remote_username = remote_ap
        .preferred_username
        .unwrap_or_else(|| follower_uri.rsplit('/').next().unwrap_or("unknown").to_string());
    let remote_display_name = remote_ap.name.unwrap_or_else(|| remote_username.clone());
    let remote_domain = follower_uri.split('/').nth(2).unwrap_or(follower_uri).to_string();

    // リモートアクターを actors テーブルに upsert
    let now = chrono::Utc::now();
    let new_id = generate_snowflake_id(now);

    let remote_row = sqlx::query(
        "INSERT INTO actors (id, actor_type, ap_uri, ap_inbox_url, username, domain, display_name, created_at, updated_at)
         VALUES ($1, 'fedi', $2, $3, $4, $5, $6, $7, $7)
         ON CONFLICT (ap_uri) DO UPDATE
           SET ap_inbox_url  = EXCLUDED.ap_inbox_url,
               display_name  = EXCLUDED.display_name,
               updated_at    = EXCLUDED.updated_at
         RETURNING id",
    )
    .bind(new_id)
    .bind(follower_uri)
    .bind(&remote_inbox)
    .bind(&remote_username)
    .bind(&remote_domain)
    .bind(&remote_display_name)
    .bind(now)
    .fetch_one(&state.db)
    .await
    .map_err(|e| format!("リモートアクター upsert エラー: {}", e))?;

    let follower_actor_id: i64 = remote_row.try_get("id").map_err(|e| e.to_string())?;

    // follows テーブルに挿入（重複時はスキップ）
    sqlx::query(
        "INSERT INTO follows (follower_actor_id, target_actor_id)
         VALUES ($1, $2)
         ON CONFLICT (follower_actor_id, target_actor_id) DO NOTHING",
    )
    .bind(follower_actor_id)
    .bind(local_actor_id)
    .execute(&state.db)
    .await
    .map_err(|e| format!("follows INSERT エラー: {}", e))?;

    // Accept アクティビティを構築して送信
    let local_actor_uri = format!("https://{}/users/{}", state.local_domain, local_username);
    let accept_id = format!(
        "https://{}/accepts/{}",
        state.local_domain,
        generate_snowflake_id(chrono::Utc::now())
    );
    let actor_key_id = format!("{}#main-key", local_actor_uri);

    let accept = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Accept",
        "id": accept_id,
        "actor": local_actor_uri,
        "object": activity
    });
    let accept_body =
        serde_json::to_string(&accept).map_err(|e| format!("Accept シリアライズ失敗: {}", e))?;

    sign_and_post(&remote_inbox, &accept_body, &actor_key_id, &state.ap_private_key_pem).await?;

    eprintln!(
        "[Follow] {} → {} フォロー完了・Accept 送信済み",
        follower_uri, local_actor_uri
    );
    Ok(())
}

// Create(Note) を受け取り posts テーブルに保存する
async fn handle_create_note(
    activity: serde_json::Value,
    state: Arc<AppState>,
) -> Result<(), String> {
    let note = &activity["object"];
    let note_id = note["id"].as_str().ok_or("Note: id がありません")?;
    let actor_uri = activity["actor"].as_str().ok_or("Create: actor がありません")?;
    let content_html = note["content"].as_str().unwrap_or("").to_string();
    let published = note["published"].as_str().unwrap_or("");

    // 公開日時を parse して snowflake ID を生成
    let created_at = published
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap_or_else(|_| chrono::Utc::now());
    let post_id = seiran_common::generate_snowflake_id(created_at);

    // リモートアクターを upsert（未登録なら作成）
    let remote_ap = seiran_common::ap::fetch_actor(actor_uri).await?;
    let remote_inbox = remote_ap.inbox.clone().unwrap_or_default();
    let remote_username = remote_ap
        .preferred_username
        .clone()
        .unwrap_or_else(|| actor_uri.rsplit('/').next().unwrap_or("unknown").to_string());
    let remote_display_name = remote_ap.name.clone().unwrap_or_else(|| remote_username.clone());
    let remote_domain = actor_uri.split('/').nth(2).unwrap_or("").to_string();

    let now = chrono::Utc::now();
    let new_actor_id = seiran_common::generate_snowflake_id(now);

    let actor_row = sqlx::query(
        "INSERT INTO actors (id, actor_type, ap_uri, ap_inbox_url, username, domain, display_name, created_at, updated_at)
         VALUES ($1, 'fedi', $2, $3, $4, $5, $6, $7, $7)
         ON CONFLICT (ap_uri) DO UPDATE
           SET ap_inbox_url  = EXCLUDED.ap_inbox_url,
               display_name  = EXCLUDED.display_name,
               updated_at    = EXCLUDED.updated_at
         RETURNING id",
    )
    .bind(new_actor_id)
    .bind(actor_uri)
    .bind(&remote_inbox)
    .bind(&remote_username)
    .bind(&remote_domain)
    .bind(&remote_display_name)
    .bind(now)
    .fetch_one(&state.db)
    .await
    .map_err(|e| format!("リモートアクター upsert エラー: {}", e))?;

    let actor_id: i64 = actor_row.try_get("id").map_err(|e| e.to_string())?;

    // HTML タグを除去して本文を得る
    let body = strip_html(&content_html);

    // posts テーブルに挿入（ap_object_id が重複する場合はスキップ）
    sqlx::query(
        "INSERT INTO posts (id, actor_id, body, ap_object_id, created_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (ap_object_id) DO NOTHING",
    )
    .bind(post_id)
    .bind(actor_id)
    .bind(&body)
    .bind(note_id)
    .bind(created_at)
    .execute(&state.db)
    .await
    .map_err(|e| format!("posts INSERT エラー: {}", e))?;

    eprintln!("[Create/Note] {} から投稿を受信・保存: {}", actor_uri, note_id);
    Ok(())
}

fn strip_html(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                result.push(' ');
            }
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    // HTML エンティティを簡易変換
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// Accept(Follow) を受け取り follows.status を accepted に更新する
async fn handle_accept(
    activity: serde_json::Value,
    state: Arc<AppState>,
) -> Result<(), String> {
    // object が {type:"Follow", actor:"...", object:"..."} 形式のみ対応
    let obj = &activity["object"];
    if obj["type"].as_str() != Some("Follow") {
        return Ok(());
    }

    let local_actor_uri = obj["actor"]
        .as_str()
        .ok_or("Accept/Follow: object.actor がありません")?;
    let remote_actor_uri = activity["actor"]
        .as_str()
        .ok_or("Accept: actor がありません")?;

    // ローカルアクターを username から特定
    let suffix = format!("https://{}/users/", state.local_domain);
    let local_username = local_actor_uri
        .strip_prefix(&suffix)
        .ok_or("Accept: object.actor がローカルアクターではありません")?;

    let local_row = sqlx::query(
        "SELECT id FROM actors WHERE username = $1 AND domain = $2 AND actor_type = 'local' LIMIT 1",
    )
    .bind(local_username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?
    .ok_or_else(|| format!("ローカルアクター '{}' が見つかりません", local_username))?;

    let local_actor_id: i64 = local_row.try_get("id").map_err(|e| e.to_string())?;

    // リモートアクターを ap_uri から特定
    let remote_row = sqlx::query("SELECT id FROM actors WHERE ap_uri = $1 LIMIT 1")
        .bind(remote_actor_uri)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| format!("リモートアクター検索エラー: {}", e))?
        .ok_or_else(|| format!("リモートアクター '{}' が DB に見つかりません", remote_actor_uri))?;

    let remote_actor_id: i64 = remote_row.try_get("id").map_err(|e| e.to_string())?;

    // follows.status を accepted に更新
    let updated = sqlx::query(
        "UPDATE follows SET status = 'accepted'
         WHERE follower_actor_id = $1 AND target_actor_id = $2 AND status = 'pending'",
    )
    .bind(local_actor_id)
    .bind(remote_actor_id)
    .execute(&state.db)
    .await
    .map_err(|e| format!("follows UPDATE エラー: {}", e))?;

    eprintln!(
        "[Accept] {} → {} フォロー確定 (rows={})",
        local_actor_uri,
        remote_actor_uri,
        updated.rows_affected()
    );
    Ok(())
}

// Undo(Follow) アクティビティを処理してフォロー解除する
async fn handle_undo(
    activity: serde_json::Value,
    state: Arc<AppState>,
) -> Result<(), String> {
    let obj = &activity["object"];
    if obj["type"].as_str() != Some("Follow") {
        return Ok(());
    }

    let follower_uri = activity["actor"]
        .as_str()
        .ok_or("Undo: actor フィールドがありません")?;
    let target_uri = obj["object"]
        .as_str()
        .ok_or("Undo/Follow: object.object フィールドがありません")?;

    let local_username = target_uri
        .rsplit('/')
        .next()
        .ok_or("Undo/Follow: object.object URI からユーザー名を抽出できません")?;

    let follower_row = sqlx::query("SELECT id FROM actors WHERE ap_uri = $1 LIMIT 1")
        .bind(follower_uri)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| format!("フォロワーアクター検索エラー: {}", e))?;

    let follower_id = match follower_row {
        Some(r) => r.try_get::<i64, _>("id").map_err(|e| e.to_string())?,
        None => return Ok(()), // 既にいない場合は何もしない
    };

    let target_row = sqlx::query(
        "SELECT id FROM actors WHERE username = $1 AND domain = $2 AND actor_type = 'local' LIMIT 1",
    )
    .bind(local_username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| format!("ローカルアクター検索エラー: {}", e))?;

    let target_id = match target_row {
        Some(r) => r.try_get::<i64, _>("id").map_err(|e| e.to_string())?,
        None => return Ok(()),
    };

    sqlx::query(
        "DELETE FROM follows WHERE follower_actor_id = $1 AND target_actor_id = $2",
    )
    .bind(follower_id)
    .bind(target_id)
    .execute(&state.db)
    .await
    .map_err(|e| format!("follows DELETE エラー: {}", e))?;

    eprintln!("[Undo/Follow] {} のフォロー解除完了", follower_uri);
    Ok(())
}

// =====================================================================
// GET /.well-known/webfinger?resource=acct:username@domain
// =====================================================================

#[derive(Deserialize)]
struct WebFingerQuery {
    resource: Option<String>,
}

#[derive(Serialize)]
struct WebFingerResponse {
    subject: String,
    aliases: Vec<String>,
    links: Vec<WebFingerLink>,
}

#[derive(Serialize)]
struct WebFingerLink {
    rel: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    href: Option<String>,
}

async fn webfinger_handler(
    Query(query): Query<WebFingerQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let resource = match query.resource {
        Some(r) => r,
        None => return (StatusCode::BAD_REQUEST, "resource パラメータが必要です").into_response(),
    };

    let acct = resource.trim_start_matches("acct:");
    let parts: Vec<&str> = acct.splitn(2, '@').collect();
    if parts.len() != 2 {
        return (StatusCode::BAD_REQUEST, "resource フォーマット不正").into_response();
    }
    let (username, domain) = (parts[0], parts[1]);

    if domain != state.local_domain {
        return (StatusCode::NOT_FOUND, "このドメインは管理対象外です").into_response();
    }

    let exists = sqlx::query(
        "SELECT id FROM actors WHERE username = $1 AND domain = $2 AND actor_type = 'local' LIMIT 1",
    )
    .bind(username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await;

    match exists {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, "ユーザーが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[WebFinger] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    }

    let actor_uri = format!("https://{}/users/{}", state.local_domain, username);
    let response = WebFingerResponse {
        subject: format!("acct:{}@{}", username, state.local_domain),
        aliases: vec![actor_uri.clone()],
        links: vec![WebFingerLink {
            rel: "self".to_string(),
            mime_type: Some("application/activity+json".to_string()),
            href: Some(actor_uri),
        }],
    };

    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/jrd+json; charset=utf-8",
        )],
        Json(response),
    )
        .into_response()
}

// =====================================================================
// GET /users/:username — AP アクタードキュメント
// =====================================================================

#[derive(Serialize)]
struct ApActorDocument {
    #[serde(rename = "@context")]
    context: Vec<String>,
    id: String,
    #[serde(rename = "type")]
    actor_type: String,
    #[serde(rename = "preferredUsername")]
    preferred_username: String,
    name: String,
    inbox: String,
    outbox: String,
    followers: String,
    following: String,
    url: String,
    #[serde(rename = "publicKey")]
    public_key: ApPublicKey,
}

#[derive(Serialize)]
struct ApPublicKey {
    id: String,
    owner: String,
    #[serde(rename = "publicKeyPem")]
    public_key_pem: String,
}

async fn actor_handler(
    Path(username): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let row = sqlx::query(
        "SELECT username, display_name FROM actors
         WHERE username = $1 AND domain = $2 AND actor_type = 'local' LIMIT 1",
    )
    .bind(&username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await;

    let display_name = match row {
        Ok(Some(r)) => r
            .try_get::<Option<String>, _>("display_name")
            .ok()
            .flatten()
            .unwrap_or_else(|| username.clone()),
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            eprintln!("[Actor] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let base = format!("https://{}", state.local_domain);
    let actor_uri = format!("{}/users/{}", base, username);

    let doc = ApActorDocument {
        context: vec![
            "https://www.w3.org/ns/activitystreams".to_string(),
            "https://w3id.org/security/v1".to_string(),
        ],
        id: actor_uri.clone(),
        actor_type: "Person".to_string(),
        preferred_username: username.clone(),
        name: display_name,
        inbox: format!("{}/inbox", base),
        outbox: format!("{}/users/{}/outbox", base, username),
        followers: format!("{}/users/{}/followers", base, username),
        following: format!("{}/users/{}/following", base, username),
        url: format!("{}/@{}", base, username),
        public_key: ApPublicKey {
            id: format!("{}#main-key", actor_uri),
            owner: actor_uri,
            public_key_pem: state.ap_public_key_pem.clone(),
        },
    };

    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/activity+json",
        )],
        Json(doc),
    )
        .into_response()
}

// =====================================================================
// GET /.well-known/nodeinfo — NodeInfo ディスカバリー
// GET /nodeinfo/2.1       — NodeInfo 本体
// =====================================================================

async fn nodeinfo_discovery_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let body = serde_json::json!({
        "links": [{
            "rel": "http://nodeinfo.diaspora.software/ns/schema/2.1",
            "href": format!("https://{}/nodeinfo/2.1", state.local_domain)
        }]
    });
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        Json(body),
    )
        .into_response()
}

async fn nodeinfo_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let user_count: i64 =
        sqlx::query("SELECT COUNT(*) AS cnt FROM actors WHERE actor_type = 'local'")
            .fetch_one(&state.db)
            .await
            .and_then(|r| r.try_get("cnt"))
            .unwrap_or(0);

    let post_count: i64 = sqlx::query(
        "SELECT COUNT(*) AS cnt FROM posts
         WHERE actor_id IN (SELECT id FROM actors WHERE actor_type = 'local')
           AND deleted_at IS NULL",
    )
    .fetch_one(&state.db)
    .await
    .and_then(|r| r.try_get("cnt"))
    .unwrap_or(0);

    let body = serde_json::json!({
        "version": "2.1",
        "software": {
            "name": "seiran",
            "version": "0.1.0"
        },
        "protocols": ["activitypub"],
        "usage": {
            "users": {
                "total": user_count,
                "activeMonth": user_count,
                "activeHalfyear": user_count
            },
            "localPosts": post_count
        },
        "openRegistrations": true
    });

    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json; profile=\"http://nodeinfo.diaspora.software/ns/schema/2.1#\"")],
        Json(body),
    )
        .into_response()
}

// =====================================================================
// GET /users/:username/outbox — AP Outbox
// =====================================================================

#[derive(Deserialize)]
struct OutboxQuery {
    page: Option<String>,
    max_id: Option<String>,
}

async fn outbox_handler(
    Path(username): Path<String>,
    Query(query): Query<OutboxQuery>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // アクターの存在確認と投稿数取得
    let actor_row = sqlx::query(
        "SELECT a.id, COUNT(p.id) AS total
         FROM actors a
         LEFT JOIN posts p ON p.actor_id = a.id AND p.deleted_at IS NULL
         WHERE a.username = $1 AND a.domain = $2 AND a.actor_type = 'local'
         GROUP BY a.id
         LIMIT 1",
    )
    .bind(&username)
    .bind(&state.local_domain)
    .fetch_optional(&state.db)
    .await;

    let (actor_id, total_items): (i64, i64) = match actor_row {
        Ok(Some(r)) => (
            r.try_get("id").unwrap_or(0),
            r.try_get("total").unwrap_or(0),
        ),
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            eprintln!("[Outbox] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let base = format!("https://{}", state.local_domain);
    let outbox_uri = format!("{}/users/{}/outbox", base, username);
    let actor_uri = format!("{}/users/{}", base, username);
    let followers_uri = format!("{}/followers", actor_uri);
    let actor_key_uri = format!("{}#main-key", actor_uri);
    let _ = actor_key_uri; // Outbox items には publicKey 不要

    // ?page 無し → OrderedCollection（インデックスのみ）
    if query.page.as_deref() != Some("true") {
        let body = serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "type": "OrderedCollection",
            "id": outbox_uri,
            "totalItems": total_items,
            "first": format!("{}?page=true", outbox_uri),
            "last": format!("{}?min_id=0&page=true", outbox_uri)
        });
        return (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/activity+json")],
            Json(body),
        )
            .into_response();
    }

    // ?page=true → OrderedCollectionPage（最大 20 件）
    const PAGE_SIZE: i64 = 20;
    let max_id: Option<i64> = query.max_id.as_deref().and_then(|s| s.parse().ok());

    let rows = match max_id {
        Some(mid) => sqlx::query(
            "SELECT id, body, created_at FROM posts
             WHERE actor_id = $1 AND deleted_at IS NULL AND id < $2
             ORDER BY id DESC LIMIT $3",
        )
        .bind(actor_id)
        .bind(mid)
        .bind(PAGE_SIZE)
        .fetch_all(&state.db)
        .await,
        None => sqlx::query(
            "SELECT id, body, created_at FROM posts
             WHERE actor_id = $1 AND deleted_at IS NULL
             ORDER BY id DESC LIMIT $2",
        )
        .bind(actor_id)
        .bind(PAGE_SIZE)
        .fetch_all(&state.db)
        .await,
    };

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[Outbox] 投稿取得エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let mut ordered_items = Vec::new();
    let mut oldest_id: Option<i64> = None;

    for row in &rows {
        let post_id: i64 = match row.try_get("id") {
            Ok(v) => v,
            Err(_) => continue,
        };
        let body: String = match row.try_get("body") {
            Ok(v) => v,
            Err(_) => continue,
        };
        let created_at: chrono::DateTime<chrono::Utc> = match row.try_get("created_at") {
            Ok(v) => v,
            Err(_) => continue,
        };

        oldest_id = Some(post_id);
        let note_id = format!("{}/notes/{}", base, post_id);
        let activity_id = format!("{}/activities/{}", base, post_id);
        let published = created_at.to_rfc3339();
        let content_html = plain_to_html(&body);

        ordered_items.push(serde_json::json!({
            "type": "Create",
            "id": activity_id,
            "actor": actor_uri,
            "published": published,
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": [followers_uri],
            "object": {
                "type": "Note",
                "id": note_id,
                "attributedTo": actor_uri,
                "content": content_html,
                "published": published,
                "to": ["https://www.w3.org/ns/activitystreams#Public"],
                "cc": [followers_uri],
                "url": note_id
            }
        }));
    }

    let mut page = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "OrderedCollectionPage",
        "id": format!("{}?page=true", outbox_uri),
        "partOf": outbox_uri,
        "orderedItems": ordered_items
    });

    // 次ページリンク（取得件数が上限に達した場合）
    if rows.len() as i64 == PAGE_SIZE {
        if let Some(oid) = oldest_id {
            page["next"] = serde_json::json!(
                format!("{}?page=true&max_id={}", outbox_uri, oid)
            );
        }
    }

    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/activity+json")],
        Json(page),
    )
        .into_response()
}

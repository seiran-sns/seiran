use axum::{extract::State, http::HeaderMap, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

use seiran_common::{generate_snowflake_id, ApError};
use seiran_common::atp::fetch_atp_history;

use crate::middleware::extract_auth;
use crate::AppState;

// AppView getProfile レスポンス（フォロー時のアクター情報取得に使用）
#[derive(Deserialize)]
pub struct CreateFollowRequest {
    /// ローカルユーザー名 / `@alice@mastodon.social` / `https://...` / `did:plc:...`
    pub target: String,
}

#[derive(Deserialize)]
pub struct DeleteFollowRequest {
    pub target: String,
}

#[derive(Serialize)]
pub struct FollowResponse {
    pub status: String,
    pub target_uri: String,
}

pub async fn create_follow(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateFollowRequest>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(&headers, &state.local_auth).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };

    let t = req.target.trim().trim_start_matches('@');

    // HTTP(S) URI → Fedi AP フォロー（ATP ハンドル判定より先に弾く）
    if t.starts_with("https://") || t.starts_with("http://") {
        return follow_fedi(t, auth_user.user_id, &state).await.into_response();
    }

    // DID 形式 → Bsky ATP フォロー
    if t.starts_with("did:") {
        return follow_bsky(t, auth_user.user_id, &state).await.into_response();
    }

    // ATP ハンドル（ドット含み・@なし・http なし）→ Bsky ATP フォロー
    if t.contains('.') && !t.contains('@') {
        return follow_bsky(t, auth_user.user_id, &state).await.into_response();
    }

    // ローカルユーザー名（@ なし・ドットなし）→ ローカルフォロー
    let parts: Vec<&str> = t.splitn(2, '@').collect();
    if parts.len() == 1 {
        return follow_local(parts[0], auth_user.user_id, &state).await.into_response();
    }
    // `alice@seiran.org` → ローカルフォロー
    if parts.len() == 2 && parts[1] == state.local_domain {
        return follow_local(parts[0], auth_user.user_id, &state).await.into_response();
    }

    // Fedi リモート (`alice@mastodon.social`)
    follow_fedi(t, auth_user.user_id, &state).await.into_response()
}

pub async fn delete_follow(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<DeleteFollowRequest>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(&headers, &state.local_auth).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };

    let local_actor = match state.actors.find_local_by_user_id(auth_user.user_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return (StatusCode::NOT_FOUND, "アクターが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[unfollow] ローカルアクター取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let t = req.target.trim().trim_start_matches('@');

    // ターゲットアクターを DB から取得
    let target_actor = if t.starts_with("did:") {
        state.actors.find_by_did(t).await
    } else if t.starts_with("https://") || t.starts_with("http://") {
        state.actors.find_by_ap_uri(t).await
    } else {
        let parts: Vec<&str> = t.splitn(2, '@').collect();
        let (username, domain) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            (parts[0], state.local_domain.as_str())
        };
        state.actors.find_by_username_domain(username, domain).await
    };

    let target_actor = match target_actor {
        Ok(Some(a)) => a,
        Ok(None) => return (StatusCode::NOT_FOUND, "ターゲットが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[unfollow] ターゲット取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // フォロー関係と atp_rkey を取得
    let atp_rkey = match state.follows.find_atp_rkey(local_actor.id, target_actor.id).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[unfollow] atp_rkey 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let now = chrono::Utc::now();

    // ATP フォロー解除（atp_rkey が保存されている場合）
    if let Some(ref rkey) = atp_rkey {
        if let Err(e) = state.atp_service.commit_delete_follow(local_actor.id, rkey, now).await {
            eprintln!("[unfollow] ATP delete commit 失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("ATP コミット失敗: {}", e)).into_response();
        }
    }

    // AP Undo Follow（Fedi リモートアクター、かつローカルアクターでない場合のみ）
    if target_actor.actor_type != "local" && target_actor.actor_type != "bsky" {
        if let (Some(ap_inbox_url), Some(ap_uri)) =
            (target_actor.ap_inbox_url.as_deref(), target_actor.ap_uri.as_deref())
        {
            let local_actor_uri = format!("https://{}/users/{}", state.local_domain, local_actor.username);
            let actor_key_id = format!("{}#main-key", local_actor_uri);
            let follow_id = format!("https://{}/activities/follow/{}", state.local_domain, target_actor.id);
            let ap_private_key_pem = state.secrets.ap_private_key_pem.clone().unwrap_or_default();

            let undo_activity = json!({
                "@context": "https://www.w3.org/ns/activitystreams",
                "type": "Undo",
                "id": format!("{}/undo", follow_id),
                "actor": local_actor_uri,
                "object": {
                    "type": "Follow",
                    "id": follow_id,
                    "actor": local_actor_uri,
                    "object": ap_uri,
                }
            });

            if let Ok(body) = serde_json::to_string(&undo_activity) {
                if let Err(e) = state.ap_client.sign_and_post(ap_inbox_url, &body, &actor_key_id, &ap_private_key_pem).await {
                    eprintln!("[unfollow] AP Undo Follow 送信失敗: {}", e);
                }
            }
        }
    }

    // follows テーブルから削除
    if let Err(e) = state.follows.delete_by_actors(local_actor.id, target_actor.id).await {
        eprintln!("[unfollow] follows DELETE 失敗: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
    }

    eprintln!("[unfollow] {} → {} アンフォロー完了", local_actor.id, target_actor.id);
    Json(serde_json::json!({"status": "ok"})).into_response()
}

/// ローカルユーザーへのフォロー（ATP コミット + follows テーブル accepted）
async fn follow_local(username: &str, user_id: i64, state: &AppState) -> impl IntoResponse {
    let local_actor = match state.actors.find_local_by_user_id(user_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return (StatusCode::NOT_FOUND, "アクターが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[follow/local] ローカルアクター取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let target_actor = match state.actors.find_by_username_domain(username, &state.local_domain).await {
        Ok(Some(a)) => a,
        Ok(None) => return (StatusCode::NOT_FOUND, "ターゲットユーザーが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[follow/local] ターゲット取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    if local_actor.id == target_actor.id {
        return (StatusCode::BAD_REQUEST, "自分自身はフォローできません").into_response();
    }

    let target_did = match target_actor.at_did.as_deref() {
        Some(d) => d.to_string(),
        None => return (StatusCode::BAD_REQUEST, "ターゲットに ATP DID がありません").into_response(),
    };

    let now = chrono::Utc::now();
    let rkey = match state.atp_service.commit_follow(local_actor.id, &target_did, now).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[follow/local] ATP commit 失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("ATP コミット失敗: {}", e)).into_response();
        }
    };

    if let Err(e) = state.follows.insert_accepted_bsky(local_actor.id, target_actor.id, &rkey).await {
        eprintln!("[follow/local] follows INSERT 失敗: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
    }

    eprintln!("[follow/local] {} → {} ローカルフォロー完了 (rkey={})", local_actor.id, target_actor.id, rkey);

    Json(FollowResponse {
        status: "accepted".to_string(),
        target_uri: format!("https://{}/users/{}", state.local_domain, username),
    })
    .into_response()
}

/// Bsky リモートユーザーへの ATP フォロー（DID またはハンドル）
async fn follow_bsky(actor_id_or_handle: &str, user_id: i64, state: &AppState) -> impl IntoResponse {
    let local_actor = match state.actors.find_local_by_user_id(user_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return (StatusCode::NOT_FOUND, "アクターが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[follow/bsky] ローカルアクター取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // AppView からプロフィール情報を取得（DID 解決 + アクター登録用）
    let url = format!(
        "https://public.api.bsky.app/xrpc/app.bsky.actor.getProfile?actor={}",
        urlencoding::encode(actor_id_or_handle)
    );
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BskyResp { did: String, handle: String, display_name: Option<String>, avatar: Option<String> }

    let bsky_resp = match state.http_client.get(&url).send().await {
        Ok(r) if r.status().is_success() => match r.json::<BskyResp>().await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[follow/bsky] AppView JSON 解析失敗: {}", e);
                return (StatusCode::BAD_GATEWAY, "AppView レスポンス解析失敗").into_response();
            }
        },
        Ok(r) => return (StatusCode::NOT_FOUND, format!("Bsky ユーザーが見つかりません ({})", r.status())).into_response(),
        Err(e) => {
            eprintln!("[follow/bsky] AppView 接続失敗: {}", e);
            return (StatusCode::BAD_GATEWAY, "AppView 接続失敗").into_response();
        }
    };
    let did = bsky_resp.did.clone();

    let now = chrono::Utc::now();
    let new_actor_id = generate_snowflake_id(now);
    let remote_actor_id = match state.actors.upsert_remote_bsky(
        new_actor_id, &did, &bsky_resp.handle, bsky_resp.display_name.as_deref(), bsky_resp.avatar.as_deref(), now,
    ).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("[follow/bsky] アクター upsert 失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let rkey = match state.atp_service.commit_follow(local_actor.id, &did, now).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[follow/bsky] ATP commit 失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("ATP コミット失敗: {}", e)).into_response();
        }
    };

    if let Err(e) = state.follows.insert_accepted_bsky(local_actor.id, remote_actor_id, &rkey).await {
        eprintln!("[follow/bsky] follows INSERT 失敗: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
    }

    eprintln!("[follow/bsky] {} → {} フォロー完了 (rkey={})", local_actor.id, did, rkey);

    // バックグラウンドで過去ポストを取り込む（フォロー直後に既存投稿を表示するため）
    {
        let db = state.db.clone();
        let http = state.http_client.clone();
        let did_clone = did.clone();
        tokio::spawn(async move {
            backfill_bsky_posts(&db, &http, &did_clone, remote_actor_id).await;
        });
    }

    Json(FollowResponse {
        status: "accepted".to_string(),
        target_uri: format!("at://{}", did),
    })
    .into_response()
}

/// Fedi リモートユーザーへの AP フォロー
async fn follow_fedi(target: &str, user_id: i64, state: &AppState) -> impl IntoResponse {
    let local_actor = match state.actors.find_local_by_user_id(user_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return (StatusCode::NOT_FOUND, "アクターが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[follow/fedi] ローカルアクター取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let target_uri = match resolve_target_uri(state, target).await {
        Ok(uri) => uri,
        Err(e) => {
            eprintln!("[follow/fedi] ターゲット解決失敗: {}", e);
            return (StatusCode::BAD_REQUEST, format!("ターゲット解決失敗: {}", e)).into_response();
        }
    };

    let remote_ap = match state.ap_client.fetch_actor(&target_uri).await {
        Ok(a) => a,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("リモートアクター取得失敗: {}", e)).into_response(),
    };

    let remote_inbox = match remote_ap.inbox.as_deref() {
        Some(u) => u.to_string(),
        None => return (StatusCode::BAD_GATEWAY, "リモートアクターに inbox がありません").into_response(),
    };

    let remote_avatar_url = remote_ap.avatar_url();
    let remote_username = remote_ap
        .preferred_username
        .unwrap_or_else(|| target_uri.rsplit('/').next().unwrap_or("unknown").to_string());
    let remote_display_name = remote_ap.name.clone().unwrap_or_else(|| remote_username.clone());
    let remote_domain = target_uri.split('/').nth(2).unwrap_or("").to_string();

    let now = chrono::Utc::now();
    let new_actor_id = generate_snowflake_id(now);
    let remote_actor_id = match state.actors.upsert_remote_fedi(
        new_actor_id, &target_uri, &remote_inbox, &remote_username,
        &remote_domain, &remote_display_name, remote_avatar_url.as_deref(), now,
    ).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("[follow/fedi] リモートアクター upsert 失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    if let Err(e) = state.follows.upsert_pending(local_actor.id, remote_actor_id).await {
        eprintln!("[follow/fedi] follows INSERT 失敗: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
    }

    let local_actor_uri = format!("https://{}/users/{}", state.local_domain, local_actor.username);
    let actor_key_id = format!("{}#main-key", local_actor_uri);
    let follow_id = format!("https://{}/activities/follow/{}", state.local_domain, remote_actor_id);
    let ap_private_key_pem = state.secrets.ap_private_key_pem.clone().unwrap_or_default();

    let follow_activity = json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Follow",
        "id": follow_id,
        "actor": local_actor_uri,
        "object": target_uri
    });

    let body = match serde_json::to_string(&follow_activity) {
        Ok(b) => b,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("JSON シリアライズ失敗: {}", e)).into_response(),
    };

    if let Err(e) = state.ap_client.sign_and_post(&remote_inbox, &body, &actor_key_id, &ap_private_key_pem).await {
        eprintln!("[follow/fedi] Follow 送信失敗: {}", e);
        return (StatusCode::BAD_GATEWAY, format!("Follow 送信失敗: {}", e)).into_response();
    }

    eprintln!("[follow/fedi] {} → {} Follow 送信完了 (pending)", local_actor_uri, target_uri);

    Json(FollowResponse {
        status: "pending".to_string(),
        target_uri,
    })
    .into_response()
}

/// `@alice@mastodon.social` または `https://...` 形式のターゲットを Actor URI に解決する
async fn resolve_target_uri(state: &AppState, target: &str) -> Result<String, ApError> {
    let t = target.trim().trim_start_matches('@');

    if t.starts_with("https://") || t.starts_with("http://") {
        return Ok(t.to_string());
    }

    let parts: Vec<&str> = t.splitn(2, '@').collect();
    if parts.len() == 2 {
        return state.ap_client.resolve_webfinger(parts[0], parts[1]).await;
    }

    Err(ApError::Other(format!(
        "ターゲット形式が不正です: {}",
        target
    )))
}

/// Bsky アクターの過去ポストを AppView から取り込む（バックフィル）。
/// フォロー時・プロフィール表示時にバックグラウンドで呼ばれる。
pub(crate) async fn backfill_bsky_posts(
    db: &sqlx::PgPool,
    http: &reqwest::Client,
    did: &str,
    actor_id: i64,
) {
    match fetch_atp_history(http, did, 50, 7).await {
        Ok(posts) => {
            let count = posts.len();
            for post in &posts {
                let post_id = generate_snowflake_id(post.created_at);
                let _ = sqlx::query(
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
                .execute(db)
                .await;
            }
            eprintln!("[backfill/bsky] did={} {} 件処理", did, count);
        }
        Err(e) => eprintln!("[backfill/bsky] 失敗 did={}: {}", did, e),
    }
}

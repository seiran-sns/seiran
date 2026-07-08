use axum::{extract::State, http::HeaderMap, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

use seiran_common::{generate_snowflake_id, ApError};

use crate::middleware::extract_auth;
use crate::AppState;

// AppView getProfile レスポンス（フォロー時のアクター情報取得に使用）
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppViewProfile {
    handle: String,
    display_name: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateFollowRequest {
    /// `@alice@mastodon.social` 形式または `https://mastodon.social/users/alice` 形式
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

    // ローカルアクター取得
    let (local_actor_id, local_username) =
        match state.actors.find_local_by_user_id(auth_user.user_id).await {
            Ok(Some(a)) => (a.id, a.username),
            Ok(None) => {
                return (axum::http::StatusCode::NOT_FOUND, "アクターが見つかりません")
                    .into_response()
            }
            Err(e) => {
                eprintln!("[follow] ローカルアクター取得失敗: {}", e);
                return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "DB エラー")
                    .into_response();
            }
        };

    // DID 形式 → ATP フォローパス
    if req.target.starts_with("did:") {
        return follow_bsky(&req.target, auth_user.user_id, &state).await.into_response();
    }

    // ターゲット URI を解決（handle または URI）
    let target_uri = match resolve_target_uri(&state, &req.target).await {
        Ok(uri) => uri,
        Err(e) => {
            eprintln!("[follow] ターゲット解決失敗: {}", e);
            return (
                axum::http::StatusCode::BAD_REQUEST,
                format!("ターゲット解決失敗: {}", e),
            )
                .into_response();
        }
    };

    // 自分自身へのフォロー拒否
    if target_uri.contains(&format!("//{}/", state.local_domain)) {
        return (axum::http::StatusCode::BAD_REQUEST, "ローカルユーザーへのフォローはこのエンドポイントでは未対応です")
            .into_response();
    }

    // リモートアクタードキュメント取得
    let remote_ap = match state.ap_client.fetch_actor(&target_uri).await {
        Ok(a) => a,
        Err(e) => {
            return (
                axum::http::StatusCode::BAD_GATEWAY,
                format!("リモートアクター取得失敗: {}", e),
            )
                .into_response()
        }
    };

    let remote_inbox = match remote_ap.inbox.as_deref() {
        Some(u) => u.to_string(),
        None => {
            return (axum::http::StatusCode::BAD_GATEWAY, "リモートアクターに inbox がありません")
                .into_response()
        }
    };

    let remote_avatar_url = remote_ap.avatar_url();
    let remote_username = remote_ap
        .preferred_username
        .unwrap_or_else(|| target_uri.rsplit('/').next().unwrap_or("unknown").to_string());
    let remote_display_name = remote_ap.name.unwrap_or_else(|| remote_username.clone());
    let remote_domain = target_uri.split('/').nth(2).unwrap_or("").to_string();

    // リモートアクターを actors テーブルに upsert
    let now = chrono::Utc::now();
    let new_actor_id = generate_snowflake_id(now);

    let remote_actor_id: i64 = match state
        .actors
        .upsert_remote_fedi(
            new_actor_id,
            &target_uri,
            &remote_inbox,
            &remote_username,
            &remote_domain,
            &remote_display_name,
            remote_avatar_url.as_deref(),
            now,
        )
        .await
    {
        Ok(id) => id,
        Err(e) => {
            eprintln!("[follow] リモートアクター upsert 失敗: {}", e);
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // follows テーブルに pending で挿入（既存なら status を pending に戻す）
    if let Err(e) = state
        .follows
        .upsert_pending(local_actor_id, remote_actor_id)
        .await
    {
        eprintln!("[follow] follows INSERT 失敗: {}", e);
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
    }

    // Follow アクティビティを構築
    let local_actor_uri = format!("https://{}/users/{}", state.local_domain, local_username);
    let actor_key_id = format!("{}#main-key", local_actor_uri);
    let follow_id = format!(
        "https://{}/activities/follow/{}",
        state.local_domain, remote_actor_id
    );
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
        Err(e) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("JSON シリアライズ失敗: {}", e),
            )
                .into_response()
        }
    };

    if let Err(e) = state.ap_client.sign_and_post(&remote_inbox, &body, &actor_key_id, &ap_private_key_pem).await {
        eprintln!("[follow] Follow 送信失敗: {}", e);
        return (
            axum::http::StatusCode::BAD_GATEWAY,
            format!("Follow 送信失敗: {}", e),
        )
            .into_response();
    }

    eprintln!(
        "[follow] {} → {} Follow 送信完了 (pending)",
        local_actor_uri, target_uri
    );

    Json(FollowResponse {
        status: "pending".to_string(),
        target_uri,
    })
    .into_response()
}

/// DID をターゲットとした ATP フォローを実行する。
async fn follow_bsky(did: &str, user_id: i64, state: &AppState) -> impl IntoResponse {
    // ローカルアクター取得
    let local_actor = match state.actors.find_local_by_user_id(user_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return (StatusCode::NOT_FOUND, "アクターが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[follow/bsky] ローカルアクター取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // AppView からプロフィール情報を取得（アクター登録用）
    let url = format!(
        "https://public.api.bsky.app/xrpc/app.bsky.actor.getProfile?actor={}",
        urlencoding::encode(did)
    );
    let bsky_profile: AppViewProfile = match state.http_client.get(&url).send().await {
        Ok(r) if r.status().is_success() => match r.json().await {
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

    // Bsky リモートアクターを DB に登録（upsert）
    let now = chrono::Utc::now();
    let new_actor_id = generate_snowflake_id(now);
    let remote_actor_id = match state
        .actors
        .upsert_remote_bsky(
            new_actor_id,
            did,
            &bsky_profile.handle,
            bsky_profile.display_name.as_deref(),
            now,
        )
        .await
    {
        Ok(id) => id,
        Err(e) => {
            eprintln!("[follow/bsky] アクター upsert 失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // ATP リポに app.bsky.graph.follow レコードをコミット
    let rkey = match state.atp_service.commit_follow(local_actor.id, did, now).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[follow/bsky] ATP commit 失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("ATP コミット失敗: {}", e)).into_response();
        }
    };

    // follows テーブルに accepted で記録
    if let Err(e) = state
        .follows
        .insert_accepted_bsky(local_actor.id, remote_actor_id, &rkey)
        .await
    {
        eprintln!("[follow/bsky] follows INSERT 失敗: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
    }

    eprintln!("[follow/bsky] {} → {} フォロー完了 (rkey={})", local_actor.id, did, rkey);

    Json(FollowResponse {
        status: "accepted".to_string(),
        target_uri: format!("at://{}", did),
    })
    .into_response()
}

/// `@alice@mastodon.social` または `https://...` 形式のターゲットを Actor URI に解決する
async fn resolve_target_uri(state: &AppState, target: &str) -> Result<String, ApError> {
    let t = target.trim().trim_start_matches('@');

    // URI 形式（https://）
    if t.starts_with("https://") || t.starts_with("http://") {
        return Ok(t.to_string());
    }

    // handle 形式: `alice@mastodon.social` または `@alice@mastodon.social`
    let parts: Vec<&str> = t.splitn(2, '@').collect();
    if parts.len() == 2 {
        return state.ap_client.resolve_webfinger(parts[0], parts[1]).await;
    }

    Err(ApError::Other(format!(
        "ターゲット形式が不正です（@user@domain または https://... を指定してください）: {}",
        target
    )))
}

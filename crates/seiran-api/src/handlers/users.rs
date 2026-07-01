use axum::{extract::{Query, State}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}, Json};
use serde::{Deserialize, Serialize};

use seiran_common::repository::Actor;

use crate::middleware::extract_auth;
use crate::AppState;

#[derive(Deserialize)]
pub struct ProfileQuery {
    /// `@alice@mastodon.social` / `alice@mastodon.social` / `alice`（ローカル）
    pub q: String,
}

#[derive(Serialize)]
pub struct ProfileResponse {
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub actor_type: String,
    pub ap_uri: Option<String>,
    pub follow_status: String, // "not_following" | "pending" | "accepted"
    pub recent_posts: Vec<ProfileNote>,
}

#[derive(Serialize)]
pub struct ProfileNote {
    pub id: String,
    pub text: String,
    pub created_at: String,
}

pub async fn user_profile(
    Query(params): Query<ProfileQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // ログインユーザーの user_id（フォロー状態確認用）
    let my_user_id: Option<i64> = extract_auth(&headers, &state.local_auth)
        .await
        .ok()
        .map(|u| u.user_id);

    let q = params.q.trim().trim_start_matches('@');

    // ターゲットを解決：`user@domain` / `user`（ローカル）/ `https://...`（URI）
    let (lookup_username, lookup_domain): (String, Option<String>) =
        if q.starts_with("https://") || q.starts_with("http://") {
            // Actor URI → WebFinger などは省略し、DB で ap_uri 検索
            return lookup_by_uri(q, my_user_id, &state).await.into_response();
        } else {
            let parts: Vec<&str> = q.splitn(2, '@').collect();
            if parts.len() == 2 {
                (parts[0].to_string(), Some(parts[1].to_string()))
            } else {
                (parts[0].to_string(), None)
            }
        };

    let domain = lookup_domain
        .as_deref()
        .unwrap_or(&state.local_domain)
        .to_string();

    // DB で検索
    match state.actors.find_by_username_domain(&lookup_username, &domain).await {
        Ok(Some(actor)) => build_profile_response(actor, my_user_id, &state).await,
        Ok(None) if lookup_domain.is_some() => {
            // DB にいない → AP から取得して返す（DB には保存しない）
            fetch_remote_profile(&lookup_username, lookup_domain.as_deref().unwrap(), my_user_id, &state)
                .await
                .into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "ユーザーが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[profile] DB エラー: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response()
        }
    }
}

async fn build_profile_response(
    actor: Actor,
    my_user_id: Option<i64>,
    state: &AppState,
) -> Response {
    let actor_id = actor.id;

    // 自分の actor_id を取得
    let my_actor_id: Option<i64> = if let Some(uid) = my_user_id {
        match state.actors.find_local_by_user_id(uid).await {
            Ok(Some(a)) => Some(a.id),
            Ok(None) => None,
            Err(e) => {
                eprintln!("[profile] 自分の actor_id 取得失敗: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
            }
        }
    } else {
        None
    };

    // フォロー状態
    let follow_status = match my_actor_id {
        Some(mid) => match state.follows.find_status(mid, actor_id).await {
            Ok(Some(s)) => s,
            Ok(None) => "not_following".to_string(),
            Err(e) => {
                eprintln!("[profile] フォロー状態取得失敗: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
            }
        },
        None => "not_following".to_string(),
    };

    // 最近の投稿（最大20件）
    let post_rows = match state.posts.recent_by_actor(actor_id, 20).await {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("[profile] 最近の投稿取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let recent_posts = post_rows
        .into_iter()
        .map(|p| ProfileNote {
            id: p.id.to_string(),
            text: p.body,
            created_at: p.created_at.to_rfc3339(),
        })
        .collect();

    Json(ProfileResponse {
        username: actor.username,
        domain: actor.domain,
        display_name: actor.display_name,
        actor_type: actor.actor_type,
        ap_uri: actor.ap_uri,
        follow_status,
        recent_posts,
    })
    .into_response()
}

async fn fetch_remote_profile(
    username: &str,
    domain: &str,
    my_user_id: Option<i64>,
    state: &AppState,
) -> impl IntoResponse {
    // WebFinger → Actor ドキュメント取得
    let actor_uri = match state.ap_client.resolve_webfinger(username, domain).await {
        Ok(uri) => uri,
        Err(e) => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                format!("WebFinger 解決失敗: {}", e),
            )
                .into_response()
        }
    };

    let ap_actor = match state.ap_client.fetch_actor(&actor_uri).await {
        Ok(a) => a,
        Err(e) => {
            return (
                axum::http::StatusCode::BAD_GATEWAY,
                format!("アクター取得失敗: {}", e),
            )
                .into_response()
        }
    };

    let _ = my_user_id; // リモートの場合フォロー状態は常に not_following（DB 未登録）

    Json(ProfileResponse {
        username: ap_actor
            .preferred_username
            .unwrap_or_else(|| username.to_string()),
        domain: domain.to_string(),
        display_name: ap_actor.name,
        actor_type: "fedi".to_string(),
        ap_uri: Some(actor_uri),
        follow_status: "not_following".to_string(),
        recent_posts: vec![],
    })
    .into_response()
}

async fn lookup_by_uri(
    uri: &str,
    my_user_id: Option<i64>,
    state: &AppState,
) -> impl IntoResponse {
    match state.actors.find_by_ap_uri(uri).await {
        Ok(Some(actor)) => build_profile_response(actor, my_user_id, state).await,
        _ => (axum::http::StatusCode::NOT_FOUND, "ユーザーが見つかりません").into_response(),
    }
}

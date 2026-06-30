use axum::{extract::{Query, State}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}, Json};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use seiran_common::ap::{fetch_actor, resolve_webfinger};

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
    // ログインユーザーの actor_id（フォロー状態確認用）
    let my_actor_id: Option<i64> = extract_auth(&headers, &state.local_auth)
        .await
        .ok()
        .and_then(|u| {
            // NOTE: async block が使えないので DB は後で引く
            Some(u.user_id)
        });

    let q = params.q.trim().trim_start_matches('@');

    // ターゲットを解決：`user@domain` / `user`（ローカル）/ `https://...`（URI）
    let (lookup_username, lookup_domain): (String, Option<String>) =
        if q.starts_with("https://") || q.starts_with("http://") {
            // Actor URI → WebFinger などは省略し、DB で ap_uri 検索
            return lookup_by_uri(q, my_actor_id, &state).await.into_response();
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
    let actor_row = sqlx::query(
        "SELECT id, username, domain, display_name, actor_type, ap_uri FROM actors
         WHERE username = $1 AND domain = $2 LIMIT 1",
    )
    .bind(&lookup_username)
    .bind(&domain)
    .fetch_optional(&state.db)
    .await;

    match actor_row {
        Ok(Some(row)) => build_profile_response(row, my_actor_id, &state).await.into_response(),
        Ok(None) if lookup_domain.is_some() => {
            // DB にいない → AP から取得して返す（DB には保存しない）
            fetch_remote_profile(&lookup_username, lookup_domain.as_deref().unwrap(), my_actor_id, &state).await.into_response()
        }
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "ユーザーが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[profile] DB エラー: {}", e);
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response()
        }
    }
}

async fn build_profile_response(
    row: sqlx::postgres::PgRow,
    my_user_id: Option<i64>,
    state: &AppState,
) -> Response {
    let actor_id: i64 = match row.try_get("id") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[profile] actor id 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };
    let username: String = match row.try_get("username") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[profile] username 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };
    let domain: String = match row.try_get("domain") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[profile] domain 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };
    let display_name: Option<String> = row.try_get("display_name").ok().flatten();
    let actor_type: String = match row.try_get("actor_type") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[profile] actor_type 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };
    let ap_uri: Option<String> = row.try_get("ap_uri").ok().flatten();

    // 自分の actor_id を取得
    let my_actor_id: Option<i64> = if let Some(uid) = my_user_id {
        match sqlx::query("SELECT id FROM actors WHERE user_id = $1 AND actor_type = 'local' LIMIT 1")
            .bind(uid)
            .fetch_optional(&state.db)
            .await
        {
            Ok(Some(r)) => r.try_get("id").ok(),
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
        Some(mid) => {
            let f = match sqlx::query(
                "SELECT status FROM follows WHERE follower_actor_id = $1 AND target_actor_id = $2 LIMIT 1",
            )
            .bind(mid)
            .bind(actor_id)
            .fetch_optional(&state.db)
            .await
            {
                Ok(row) => row,
                Err(e) => {
                    eprintln!("[profile] フォロー状態取得失敗: {}", e);
                    return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
                }
            };
            match f {
                Some(r) => match r.try_get::<String, _>("status") {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("[profile] follow status 取得失敗: {}", e);
                        return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
                    }
                },
                None => "not_following".to_string(),
            }
        }
        None => "not_following".to_string(),
    };

    // 最近の投稿（最大20件）
    let post_rows = match sqlx::query(
        "SELECT id, body, created_at FROM posts
         WHERE actor_id = $1 AND deleted_at IS NULL
         ORDER BY id DESC LIMIT 20",
    )
    .bind(actor_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("[profile] 最近の投稿取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let recent_posts = post_rows
        .iter()
        .filter_map(|r| {
            Some(ProfileNote {
                id: r.try_get::<i64, _>("id").ok()?.to_string(),
                text: r.try_get("body").ok()?,
                created_at: r
                    .try_get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                    .ok()?
                    .to_rfc3339(),
            })
        })
        .collect();

    Json(ProfileResponse {
        username,
        domain,
        display_name,
        actor_type,
        ap_uri,
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
    let actor_uri = match resolve_webfinger(&state.http_client, username, domain).await {
        Ok(uri) => uri,
        Err(e) => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                format!("WebFinger 解決失敗: {}", e),
            )
                .into_response()
        }
    };

    let ap_actor = match fetch_actor(&state.http_client, &actor_uri).await {
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
    let row = sqlx::query(
        "SELECT id, username, domain, display_name, actor_type, ap_uri FROM actors
         WHERE ap_uri = $1 LIMIT 1",
    )
    .bind(uri)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(r)) => build_profile_response(r, my_user_id, state).await.into_response(),
        _ => (axum::http::StatusCode::NOT_FOUND, "ユーザーが見つかりません").into_response(),
    }
}

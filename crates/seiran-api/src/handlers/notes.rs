use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use seiran_common::repository::TimelinePost;
use seiran_common::{ap::{deliver_post_to_ap_followers, plain_to_html}, generate_snowflake_id};

use crate::AppState;
use crate::error::ApiError;
use crate::middleware::extract_auth;

#[derive(Deserialize)]
pub struct CreateNoteRequest {
    pub text: String,
}

#[derive(Serialize)]
pub struct NoteResponse {
    pub id: String,
    pub text: String,
    pub created_at: String,
    pub user: NoteUserInfo,
}

#[derive(Serialize)]
pub struct NoteUserInfo {
    pub id: i64,
    pub username: String,
    pub domain: Option<String>,
    pub display_name: Option<String>,
}

fn map_note_rows(result: Result<Vec<TimelinePost>, sqlx::Error>) -> impl IntoResponse {
    let rows = match result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[timeline] クエリ失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "TL取得に失敗しました").into_response();
        }
    };
    let notes: Vec<NoteResponse> = rows
        .into_iter()
        .map(|p| NoteResponse {
            id: p.id.to_string(),
            text: p.body,
            created_at: p.created_at.to_rfc3339(),
            user: NoteUserInfo {
                id: p.actor_id,
                username: p.username,
                domain: Some(p.domain),
                display_name: p.display_name,
            },
        })
        .collect();
    Json(notes).into_response()
}

#[derive(Deserialize)]
pub struct TimelineQuery {
    pub limit: Option<i64>,
    pub until_id: Option<String>,
    pub since_id: Option<String>,
}

pub async fn create_note(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateNoteRequest>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(&headers, &state.local_auth).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };

    if req.text.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "text は空にできません").into_response();
    }

    let (actor_id, username) = match state.actors.find_local_by_user_id(auth_user.user_id).await {
        Ok(Some(a)) => (a.id, a.username),
        Ok(None) => return (StatusCode::NOT_FOUND, "アクターが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[create_note] アクター取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let now = chrono::Utc::now();
    let post_id = generate_snowflake_id(now);

    let ap_object_id = format!("https://{}/notes/{}", state.local_domain, post_id);

    if let Err(e) = state
        .posts
        .insert(post_id, actor_id, &req.text, &ap_object_id, now)
        .await
    {
        eprintln!("[create_note] INSERT 失敗: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "投稿の保存に失敗しました").into_response();
    }

    if let Err(e) = state.atp_service.commit_post(actor_id, post_id, &req.text, now).await {
        eprintln!("[create_note] ATP コミット失敗（投稿は保存済み）: {}", e);
    }

    // AP フォロワーへ非同期配送
    {
        let db = state.db.clone();
        let local_domain = state.local_domain.clone();
        let ap_private_key_pem = state
            .secrets
            .ap_private_key_pem
            .clone()
            .unwrap_or_default();
        let ap_client = state.ap_client.clone();
        tokio::spawn(async move {
            if let Err(e) =
                deliver_post_to_ap_followers(&ap_client, &db, post_id, actor_id, &local_domain, &ap_private_key_pem)
                    .await
            {
                eprintln!("[create_note] AP 配送エラー: {}", e);
            }
        });
    }

    Json(NoteResponse {
        id: post_id.to_string(),
        text: req.text,
        created_at: now.to_rfc3339(),
        user: NoteUserInfo { id: auth_user.user_id, username, domain: None, display_name: None },
    })
    .into_response()
}

pub async fn home_timeline(
    Query(q): Query<TimelineQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(&headers, &state.local_auth).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };

    let actor_id: i64 = match state.actors.find_local_by_user_id(auth_user.user_id).await {
        Ok(Some(a)) => a.id,
        Ok(None) => return (StatusCode::NOT_FOUND, "アクターが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[home_timeline] アクター取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let limit = q.limit.unwrap_or(30).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = state
        .posts
        .home_timeline(actor_id, limit, until_id, since_id)
        .await;

    map_note_rows(rows).into_response()
}

pub async fn local_timeline(
    Query(q): Query<TimelineQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(20).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = state.posts.local_timeline(limit, until_id, since_id).await;

    map_note_rows(rows).into_response()
}

/// フロントエンド向け: GET /api/notes/:id
pub async fn get_note(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<NoteResponse>, ApiError> {
    let post_id: i64 = id.parse().map_err(|_| ApiError::NotFound("NOT_FOUND"))?;
    let post = state
        .posts
        .find_by_id(post_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?;
    Ok(Json(NoteResponse {
        id: post.id.to_string(),
        text: post.body,
        created_at: post.created_at.to_rfc3339(),
        user: NoteUserInfo {
            id: post.actor_id,
            username: post.username,
            domain: Some(post.domain),
            display_name: post.display_name,
        },
    }))
}

/// ActivityPub 向け: GET /notes/:id
/// nginx が Accept: application/activity+json のリクエストのみここへ転送する。
pub async fn get_note_ap(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let post_id: i64 = match id.parse() {
        Ok(i) => i,
        Err(_) => return (StatusCode::NOT_FOUND, "").into_response(),
    };

    let post = match state.posts.find_by_id(post_id).await {
        Ok(Some(p)) => p,
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            eprintln!("[get_note_ap] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
        }
    };

    // ローカルポストのみ AP として提供する
    if post.domain != state.local_domain {
        return (StatusCode::NOT_FOUND, "").into_response();
    }

    let actor_uri = format!("https://{}/users/{}", state.local_domain, post.username);
    let note_id = format!("https://{}/notes/{}", state.local_domain, post.id);
    let content_html = plain_to_html(&post.body);

    let ap_note = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Note",
        "id": note_id,
        "url": note_id,
        "attributedTo": actor_uri,
        "content": content_html,
        "published": post.created_at.to_rfc3339(),
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [format!("{}/followers", actor_uri)],
    });

    (
        [(axum::http::header::CONTENT_TYPE, "application/activity+json; charset=utf-8")],
        Json(ap_note),
    )
        .into_response()
}

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use seiran_common::{ap::deliver_post_to_ap_followers, generate_snowflake_id};

use crate::AppState;
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

    let actor_row = sqlx::query(
        "SELECT id, username FROM actors WHERE user_id = $1 AND actor_type = 'local' LIMIT 1",
    )
    .bind(auth_user.user_id)
    .fetch_optional(&state.db)
    .await;

    let (actor_id, username) = match actor_row {
        Ok(Some(r)) => {
            let id: i64 = match r.try_get("id") {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[create_note] actor id 取得失敗: {}", e);
                    return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
                }
            };
            let name: String = match r.try_get("username") {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[create_note] username 取得失敗: {}", e);
                    return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
                }
            };
            (id, name)
        }
        _ => return (StatusCode::NOT_FOUND, "アクターが見つかりません").into_response(),
    };

    let now = chrono::Utc::now();
    let post_id = generate_snowflake_id(now);

    let ap_object_id = format!("https://{}/notes/{}", state.local_domain, post_id);

    if let Err(e) = sqlx::query(
        "INSERT INTO posts (id, actor_id, body, ap_object_id, created_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(post_id)
    .bind(actor_id)
    .bind(&req.text)
    .bind(&ap_object_id)
    .bind(now)
    .execute(&state.db)
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
        tokio::spawn(async move {
            if let Err(e) =
                deliver_post_to_ap_followers(&db, post_id, actor_id, &local_domain, &ap_private_key_pem)
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
        user: NoteUserInfo { id: auth_user.user_id, username },
    })
    .into_response()
}

pub async fn local_timeline(
    Query(q): Query<TimelineQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(20).min(100);
    let until_id: Option<i64> = q.until_id.as_deref().and_then(|s| s.parse().ok());
    let since_id: Option<i64> = q.since_id.as_deref().and_then(|s| s.parse().ok());

    let rows = match (until_id, since_id) {
        (Some(uid), _) => {
            sqlx::query(
                "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username
                 FROM posts p
                 JOIN actors a ON a.id = p.actor_id
                 WHERE a.actor_type = 'local' AND p.deleted_at IS NULL AND p.id < $1
                 ORDER BY p.id DESC
                 LIMIT $2",
            )
            .bind(uid)
            .bind(limit)
            .fetch_all(&state.db)
            .await
        }
        (_, Some(sid)) => {
            sqlx::query(
                "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username
                 FROM posts p
                 JOIN actors a ON a.id = p.actor_id
                 WHERE a.actor_type = 'local' AND p.deleted_at IS NULL AND p.id > $1
                 ORDER BY p.id DESC
                 LIMIT $2",
            )
            .bind(sid)
            .bind(limit)
            .fetch_all(&state.db)
            .await
        }
        _ => {
            sqlx::query(
                "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username
                 FROM posts p
                 JOIN actors a ON a.id = p.actor_id
                 WHERE a.actor_type = 'local' AND p.deleted_at IS NULL
                 ORDER BY p.id DESC
                 LIMIT $1",
            )
            .bind(limit)
            .fetch_all(&state.db)
            .await
        }
    };

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[local_timeline] クエリ失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "TL取得に失敗しました").into_response();
        }
    };

    let notes: Vec<NoteResponse> = match rows
        .iter()
        .map(|r| -> Result<NoteResponse, sqlx::Error> {
            let id: i64 = r.try_get("id")?;
            let text: String = r.try_get("body")?;
            let created_at: chrono::DateTime<chrono::Utc> = r.try_get("created_at")?;
            let actor_id: i64 = r.try_get("actor_id")?;
            let username: String = r.try_get("username")?;
            Ok(NoteResponse {
                id: id.to_string(),
                text,
                created_at: created_at.to_rfc3339(),
                user: NoteUserInfo { id: actor_id, username },
            })
        })
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(notes) => notes,
        Err(e) => {
            eprintln!("[local_timeline] 行マッピング失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "TL取得に失敗しました").into_response();
        }
    };

    Json(notes).into_response()
}

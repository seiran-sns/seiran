use axum::{extract::{Query, State}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}, Json};
use serde::{Deserialize, Serialize};

use seiran_common::repository::Actor;

use crate::error::ApiError;
use crate::handlers::notes::{
    fetch_attachments_map, fetch_reactions_map, to_note_response, NoteResponse,
};
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
    pub at_did: Option<String>,
    pub bio: Option<String>,
    pub follow_status: String, // "not_following" | "pending" | "accepted"
    /// 最近の投稿。タイムラインと同じ NoteCard で描画するため NoteResponse で返す（#43）。
    pub recent_posts: Vec<NoteResponse>,
    // 7.3 拡張メタデータ（ブリッジ介入・魂の結合判定）
    /// このアクターがブリッジ（影武者）の場合、本尊アクターのハンドル（`user@domain`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_real_handle: Option<String>,
    /// 本尊が属するプロトコル（`fedi` / `bsky` など）。フロントの導線アイコンに使用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_protocol: Option<String>,
    /// リモート seiran ユーザーと魂の結合（ペアリング）済みか。
    pub is_paired: bool,
}

// AppView `app.bsky.actor.getProfile` レスポンスの必要フィールド
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppViewGetProfileResp {
    did: String,
    handle: String,
    display_name: Option<String>,
    description: Option<String>,
}

/// Bsky AppView からプロフィールを取得して ProfileResponse を返す。
/// `actor` はハンドル（`alice.bsky.social`）または DID（`did:plc:...`）。
async fn fetch_bsky_profile_from_appview(actor: &str, state: &AppState) -> Response {
    let url = format!(
        "https://public.api.bsky.app/xrpc/app.bsky.actor.getProfile?actor={}",
        urlencoding::encode(actor)
    );

    let resp = match state.http_client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[profile/bsky] AppView 接続失敗: {}", e);
            return (StatusCode::BAD_GATEWAY, "AppView 接続失敗").into_response();
        }
    };

    if !resp.status().is_success() {
        return (StatusCode::NOT_FOUND, "Bsky ユーザーが見つかりません").into_response();
    }

    let bsky: AppViewGetProfileResp = match resp.json().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[profile/bsky] AppView JSON 解析失敗: {}", e);
            return (StatusCode::BAD_GATEWAY, "AppView レスポンス解析失敗").into_response();
        }
    };

    Json(ProfileResponse {
        username: bsky.handle,
        domain: String::new(),
        display_name: bsky.display_name,
        actor_type: "bsky".to_string(),
        ap_uri: None,
        at_did: Some(bsky.did),
        bio: bsky.description,
        follow_status: "not_following".to_string(),
        recent_posts: vec![],
        bridge_real_handle: None,
        bridge_protocol: None,
        is_paired: false,
    })
    .into_response()
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
        } else if q.starts_with("did:") {
            // DID 形式 → DB で検索し、なければ AppView へ
            return match state.actors.find_by_did(q).await {
                Ok(Some(actor)) => build_profile_response(actor, my_user_id, &state).await,
                Ok(None) => fetch_bsky_profile_from_appview(q, &state).await,
                Err(e) => {
                    eprintln!("[profile] DB エラー: {}", e);
                    (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response()
                }
            };
        } else if q.contains('.') && !q.contains('@') {
            // ドット含み・@なし → ATP ハンドル（alice.bsky.social 等）
            // DB に at_did でアクターが登録済みの場合は DB から返す（未来の ATP フォロー実装に備え）
            return fetch_bsky_profile_from_appview(q, &state).await;
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

    // 最近の投稿（最大20件）。タイムラインと同じ NoteCard で描画するため、
    // アクター情報・添付・リアクションを含む NoteResponse で返す（#43）。
    let post_rows = match state.posts.timeline_by_actor(actor_id, 20).await {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("[profile] 最近の投稿取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };
    let post_ids: Vec<i64> = post_rows.iter().map(|p| p.id).collect();
    let mut att_map = fetch_attachments_map(&state.db, &post_ids).await;
    let rmap = fetch_reactions_map(&state.db, &post_ids).await;
    let recent_posts: Vec<NoteResponse> = post_rows
        .into_iter()
        .map(|p| {
            let id = p.id;
            let mut nr = to_note_response(p, att_map.remove(&id).unwrap_or_default());
            nr.reactions = rmap.get(&id).cloned().unwrap_or_default();
            nr
        })
        .collect();

    // 本尊（ブリッジの実体）解決: bridge_real_actor_id が埋まっていれば、
    // その本尊アクターのハンドルとプロトコルをフロントの「本尊ワープ」導線に渡す。
    let (bridge_real_handle, bridge_protocol) = match actor.bridge_real_actor_id {
        Some(real_id) => match state.actors.find_by_id(real_id).await {
            Ok(Some(real)) => {
                let handle = if real.domain == state.local_domain {
                    format!("@{}", real.username)
                } else {
                    format!("@{}@{}", real.username, real.domain)
                };
                let proto = if real.at_did.is_some() { "bsky" } else { "fedi" };
                (Some(handle), Some(proto.to_string()))
            }
            _ => (None, None),
        },
        None => (None, None),
    };

    Json(ProfileResponse {
        username: actor.username,
        domain: actor.domain,
        display_name: actor.display_name,
        actor_type: actor.actor_type,
        ap_uri: actor.ap_uri,
        at_did: actor.at_did,
        bio: actor.bio,
        follow_status,
        recent_posts,
        bridge_real_handle,
        bridge_protocol,
        is_paired: actor.seiran_pair_actor_id.is_some(),
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
        at_did: None,
        bio: ap_actor.summary,
        follow_status: "not_following".to_string(),
        recent_posts: vec![],
        bridge_real_handle: None,
        bridge_protocol: None,
        is_paired: false,
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

// =====================================================================
// PATCH /api/users/profile — プロフィール更新
// =====================================================================

#[derive(Deserialize)]
pub struct UpdateProfileRequest {
    pub display_name: Option<String>,
    /// 自己紹介。未指定なら現在値を保持、空文字なら空に更新。
    pub bio: Option<String>,
    /// `None` = フィールド未指定（現在値を保持）
    /// `Some(None)` = null を明示（解除）
    /// `Some(Some(id))` = メディア ID を設定
    #[serde(default)]
    pub avatar_media_id: Option<Option<i64>>,
    #[serde(default)]
    pub banner_media_id: Option<Option<i64>>,
}

#[derive(Serialize)]
pub struct UpdateProfileResponse {
    pub username: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_media_id: Option<i64>,
    pub banner_media_id: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct ActorProfileRow {
    username: String,
    display_name: Option<String>,
    bio: Option<String>,
    avatar_media_id: Option<i64>,
    banner_media_id: Option<i64>,
}

pub async fn update_profile(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<UpdateProfileRequest>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(&headers, &state.local_auth).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };

    // 現在のプロフィールを取得
    let current = match sqlx::query_as::<_, ActorProfileRow>(
        "SELECT username, display_name, bio, avatar_media_id, banner_media_id \
         FROM actors WHERE user_id = $1 AND actor_type = 'local' LIMIT 1",
    )
    .bind(auth_user.user_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "アクターが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[update_profile] SELECT 失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // リクエストで指定されたフィールドのみ上書き
    let new_display_name: Option<String> = if req.display_name.is_some() {
        req.display_name
    } else {
        current.display_name
    };
    let new_bio: Option<String> = if req.bio.is_some() {
        req.bio
    } else {
        current.bio
    };
    let new_avatar_media_id: Option<i64> = match req.avatar_media_id {
        None => current.avatar_media_id,
        Some(v) => v,
    };
    let new_banner_media_id: Option<i64> = match req.banner_media_id {
        None => current.banner_media_id,
        Some(v) => v,
    };

    // UPDATE
    if let Err(e) = sqlx::query(
        "UPDATE actors \
         SET display_name = $1, bio = $2, avatar_media_id = $3, banner_media_id = $4, updated_at = NOW() \
         WHERE user_id = $5 AND actor_type = 'local'",
    )
    .bind(&new_display_name)
    .bind(&new_bio)
    .bind(new_avatar_media_id)
    .bind(new_banner_media_id)
    .bind(auth_user.user_id)
    .execute(&state.db)
    .await
    {
        eprintln!("[update_profile] UPDATE 失敗: {}", e);
        return ApiError::Internal(e.to_string()).into_response();
    }

    Json(UpdateProfileResponse {
        username: current.username,
        display_name: new_display_name,
        bio: new_bio,
        avatar_media_id: new_avatar_media_id,
        banner_media_id: new_banner_media_id,
    })
    .into_response()
}

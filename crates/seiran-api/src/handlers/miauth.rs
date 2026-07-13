use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::middleware::extract_auth;
use crate::AppState;

/// MiAuth セッションの認可状態。認可（`POST /api/miauth/:session_id/authorize`）が
/// 成立して初めて `token`/`user_id`/`username` が埋まる。
#[derive(Debug, Clone, Default)]
pub struct MiAuthSession {
    pub token: Option<String>,
    pub user_id: Option<i64>,
    pub username: Option<String>,
}

#[derive(Deserialize)]
pub struct MiAuthQuery {
    pub name: String,
    pub callback: Option<String>,
}

#[derive(Deserialize)]
pub struct CheckRequest {
    pub session: String,
}

#[derive(Serialize)]
pub struct CheckResponseUser {
    pub id: String,
    pub name: String,
    pub username: String,
    pub host: Option<String>,
    #[serde(rename = "avatarUrl")]
    pub avatar_url: Option<String>,
}

#[derive(Serialize)]
pub struct CheckResponse {
    pub ok: bool,
    pub token: String,
    pub user: CheckResponseUser,
}

/// [MED-02] callback URL が安全なリダイレクト先かどうかを検証する。
///
/// MiAuth のネイティブアプリクライアント（Aria 等）は `aria://aria/miauth` のような
/// カスタム URI スキームをコールバックに使う（Doc3 §5.2 に例示済み）。これは OS の
/// Intent ディスパッチであってネットワーク到達性の懸念（SSRF・内部ネットワークへの誘導）が
/// 無いため許可する。`https` はホスト検証（localhost・プライベート IP を拒否）した上で許可、
/// `http`（平文でセッション ID が漏れる）とスクリプト実行系スキームは明示的に拒否する。
fn is_valid_callback(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    match parsed.scheme() {
        "https" => {
            let Some(host) = parsed.host_str() else {
                return false;
            };
            !(host == "localhost" || host.starts_with("127.") || host.starts_with("192.168.")
                || host.starts_with("10.") || host == "::1" || host == "[::1]")
        }
        "http" | "javascript" | "data" | "vbscript" | "file" => false,
        // それ以外はネイティブアプリのカスタム URI スキームとして許可する。
        _ => true,
    }
}

/// `GET /miauth/:session_id`
///
/// Aria 等のクライアントが最初に開く URL。認可画面自体は自社フロントエンド（SPA）側に
/// 統一したため（既存のログイン・タイムライン等の画面と一貫させるため）、ここでは
/// callback URL の検証だけ行い、`name`/`callback` を引き継いで `/connect/:session_id`
/// （フロントエンドのルート）へ 303 リダイレクトするだけの薄い入口にする。
/// ログイン要否の判定・認可アクション自体は SPA 側が `RequireAuth` と
/// `POST /api/miauth/:session_id/authorize`（Bearer 認証）で行う。
pub async fn miauth_page(Path(session_id): Path<String>, Query(query): Query<MiAuthQuery>) -> impl IntoResponse {
    // [MED-02] callback URL を事前に検証する
    if let Some(ref cb) = query.callback {
        if !is_valid_callback(cb) {
            return (StatusCode::BAD_REQUEST, "無効なコールバック URL です").into_response();
        }
    }

    let mut redirect_url = format!("/connect/{}?name={}", session_id, urlencoding::encode(&query.name));
    if let Some(ref cb) = query.callback {
        redirect_url.push_str(&format!("&callback={}", urlencoding::encode(cb)));
    }
    Redirect::to(&redirect_url).into_response()
}

/// `POST /api/miauth/:session_id/authorize`
///
/// SPA の認可確認画面（ログイン済みユーザーのみ到達できる）から、既存の
/// `Authorization: Bearer` 認証で呼ばれる。CSRF トークンの手当ては不要
/// （同一オリジンの JS が明示的に付与したヘッダーのみで認証するため、ブラウザが
/// 自動送信する Cookie 前提の CSRF とは脅威モデルが異なる）。
pub async fn miauth_authorize(
    Path(session_id): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let auth_user = match extract_auth(&headers, &state.local_auth).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };

    let username = match state.actors.find_local_by_user_id(auth_user.user_id).await {
        Ok(Some(a)) => a.username,
        Ok(None) => return (StatusCode::NOT_FOUND, "ユーザーが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[miauth] ユーザー検索失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let token = format!("miauth-token-{}", Uuid::new_v4());
    let mut map = state.miauth_sessions.write().await;
    map.insert(
        session_id,
        MiAuthSession {
            token: Some(token),
            user_id: Some(auth_user.user_id),
            username: Some(username),
        },
    );

    Json(serde_json::json!({ "ok": true })).into_response()
}

/// Misskey 互換クライアント（Aria 等）が使用するパスベース check エンドポイント。
/// `POST /api/miauth/{session_id}/check`（ボディなし）
pub async fn miauth_check_by_path(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    miauth_check_inner(&session_id, &state).await
}

/// seiran 独自フロントエンドが使用するボディベース check エンドポイント（後方互換）。
/// `POST /api/miauth/check` + `{"session": "..."}`
pub async fn miauth_check(
    State(state): State<AppState>,
    Json(payload): Json<CheckRequest>,
) -> impl IntoResponse {
    miauth_check_inner(&payload.session, &state).await
}

async fn miauth_check_inner(session_id: &str, state: &AppState) -> Response {
    let map = state.miauth_sessions.read().await;

    if let Some(session) = map.get(session_id) {
        if let (Some(token), Some(user_id), Some(username)) =
            (&session.token, &session.user_id, &session.username)
        {
            let res = CheckResponse {
                ok: true,
                token: token.clone(),
                user: CheckResponseUser {
                    id: user_id.to_string(),
                    name: username.clone(),
                    username: username.clone(),
                    host: None,
                    avatar_url: None,
                },
            };
            return Json(res).into_response();
        }
    }

    (StatusCode::BAD_REQUEST, "Invalid or unauthorized session").into_response()
}

#[cfg(test)]
mod tests {
    use super::is_valid_callback;

    #[test]
    fn valid_callback_https() {
        assert!(is_valid_callback("https://app.example.com/callback"));
    }

    #[test]
    fn invalid_callback_http() {
        assert!(!is_valid_callback("http://app.example.com/callback"));
    }

    #[test]
    fn invalid_callback_localhost() {
        assert!(!is_valid_callback("https://localhost/callback"));
        assert!(!is_valid_callback("https://127.0.0.1/callback"));
    }

    #[test]
    fn invalid_callback_private_ip() {
        assert!(!is_valid_callback("https://192.168.1.1/callback"));
        assert!(!is_valid_callback("https://10.0.0.1/callback"));
    }

    #[test]
    fn invalid_callback_malformed() {
        assert!(!is_valid_callback("not-a-url"));
        assert!(!is_valid_callback(""));
    }

    #[test]
    fn valid_callback_native_app_custom_scheme() {
        // Aria 等のネイティブアプリが使うカスタム URI スキーム（Doc3 §5.2 に例示済み）。
        assert!(is_valid_callback("aria://aria/miauth"));
        assert!(is_valid_callback("miria://callback"));
    }

    #[test]
    fn invalid_callback_script_schemes() {
        assert!(!is_valid_callback("javascript:alert(1)"));
        assert!(!is_valid_callback("data:text/html,<script>alert(1)</script>"));
        assert!(!is_valid_callback("file:///etc/passwd"));
    }
}

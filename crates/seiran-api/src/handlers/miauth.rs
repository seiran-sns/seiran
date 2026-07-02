use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::middleware::extract_auth;
use crate::AppState;

#[derive(Debug, Clone)]
pub struct MiAuthSession {
    #[allow(dead_code)]
    pub app_name: String,
    pub redirect_uri: Option<String>,
    pub token: Option<String>,
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub csrf_token: String,
}

#[derive(Deserialize)]
pub struct MiAuthQuery {
    pub name: String,
    pub callback: Option<String>,
}

#[derive(Deserialize)]
pub struct AuthorizeForm {
    pub csrf_token: String,
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

/// [MED-02] callback URL が安全なリダイレクト先かどうかを検証する
/// HTTPS スキームのみ許可し、localhost・IP アドレスは拒否する
fn is_valid_callback(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    let host = match parsed.host_str() {
        Some(h) => h,
        None => return false,
    };
    if host == "localhost" || host.starts_with("127.") || host.starts_with("192.168.")
        || host.starts_with("10.") || host == "::1" || host == "[::1]"
    {
        return false;
    }
    true
}

pub async fn miauth_page(
    Path(session_id): Path<String>,
    Query(query): Query<MiAuthQuery>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // [MED-02] callback URL を事前に検証する
    if let Some(ref cb) = query.callback {
        if !is_valid_callback(cb) {
            return (StatusCode::BAD_REQUEST, "無効なコールバック URL です").into_response();
        }
    }

    // Authorization ヘッダーが存在すれば JWT を検証してユーザー ID を取得する
    let user_id = extract_auth(&headers, &state.local_auth)
        .await
        .ok()
        .map(|u| u.user_id);

    // 未ログイン時はログイン画面へリダイレクト
    if user_id.is_none() {
        let mut return_url = format!("/miauth/{}?name={}", session_id, urlencoding::encode(&query.name));
        if let Some(ref cb) = query.callback {
            return_url.push_str(&format!("&callback={}", urlencoding::encode(cb)));
        }
        let login_url = format!("/login?redirect={}", urlencoding::encode(&return_url));
        return Redirect::to(&login_url).into_response();
    }

    // [MED-03] CSRF トークンを生成してセッションに保存する
    let csrf_token = Uuid::new_v4().to_string();

    let mut map = state.miauth_sessions.write().await;
    map.insert(
        session_id.clone(),
        MiAuthSession {
            app_name: query.name.clone(),
            redirect_uri: query.callback.clone(),
            token: None,
            user_id,
            username: None,
            csrf_token: csrf_token.clone(),
        },
    );

    // [MED-01] クエリパラメータ name を HTML エスケープしてから埋め込む
    let escaped_name = html_escape::encode_text(&query.name);
    let escaped_session_id = html_escape::encode_text(&session_id);

    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
  <title>seiran - MiAuth 認可</title>
  <style>
    body {{ font-family: sans-serif; display: flex; justify-content: center; align-items: center;
            height: 100vh; margin: 0; background: #121214; color: #e1e1e6; }}
    .card {{ background: #202024; padding: 30px; border-radius: 8px; max-width: 400px; text-align: center; }}
    h2 {{ margin-top: 0; color: #fff; }}
    button {{ background: #4f46e5; color: white; border: none; padding: 10px 20px;
              border-radius: 4px; font-size: 16px; cursor: pointer; margin-top: 20px; }}
  </style>
</head>
<body>
  <div class="card">
    <h2>アプリ連携の認可</h2>
    <p>アプリ <strong>{}</strong> が seiran アカウントへのアクセスを求めています。</p>
    <form action="/miauth/{}/authorize" method="POST">
      <input type="hidden" name="csrf_token" value="{}">
      <button type="submit">連携を承認する</button>
    </form>
  </div>
</body>
</html>"#,
        escaped_name, escaped_session_id, csrf_token
    );

    Html(html).into_response()
}

pub async fn miauth_authorize(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    Form(form): Form<AuthorizeForm>,
) -> impl IntoResponse {
    // [MED-03] CSRF トークン検証（セッションのトークンとフォームのトークンを照合）
    let (user_id, redirect_uri) = {
        let map = state.miauth_sessions.read().await;
        match map.get(&session_id) {
            Some(session) => {
                if session.csrf_token != form.csrf_token {
                    return (StatusCode::FORBIDDEN, "CSRF トークンが無効です").into_response();
                }
                match session.user_id {
                    Some(id) => (id, session.redirect_uri.clone()),
                    None => {
                        return (
                            StatusCode::UNAUTHORIZED,
                            "認証が必要です。先にログインしてから認可してください。",
                        )
                            .into_response()
                    }
                }
            }
            None => {
                return (StatusCode::NOT_FOUND, "セッションが見つかりません").into_response()
            }
        }
    };

    // Phase 2: DB からユーザー名を取得（ロック不保持）
    let username = match state.actors.find_local_by_user_id(user_id).await {
        Ok(Some(a)) => a.username,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "ユーザーが見つかりません").into_response()
        }
        Err(e) => {
            eprintln!("[miauth] ユーザー検索失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // Phase 3: セッションにトークンとユーザー名を保存し、リダイレクト（書き込みロック）
    let mut map = state.miauth_sessions.write().await;
    if let Some(session) = map.get_mut(&session_id) {
        let token = format!("miauth-token-{}", Uuid::new_v4());
        session.token = Some(token);
        session.username = Some(username);

        if let Some(ref callback) = redirect_uri {
            // [MED-02] リダイレクト直前に再度検証（セッション保存時点でのチェック済みだが念のため）
            if is_valid_callback(callback) {
                let redirect_url = if callback.contains('?') {
                    format!("{}&session={}", callback, session_id)
                } else {
                    format!("{}?session={}", callback, session_id)
                };
                return Redirect::to(&redirect_url).into_response();
            }
        }
    }

    Html("<h3>認可されました。アプリに戻ってください。</h3>").into_response()
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
}

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Redirect, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::middleware::extract_auth;
use crate::AppState;

/// MiAuth セッションの認可状態。認可（`POST /api/miauth/:session_id/authorize`）が
/// 成立して初めて `token`/`user_id`/`username` が埋まる。
/// `user_id` は `users.id` ではなく **`actors.id`**（Misskey 互換レイヤーの他所
/// （`/api/i`・`/api/notes/show` 等）と同じ「ユーザーID」の意味）を保持する。
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

/// Misskey クライアント（Aria 等）は `misskey_dart` の `UserDetailedNotMe.fromJson` で
/// この JSON をパースする。そのモデルは `id`/`username`/`isBot`/`isCat`/`createdAt`/
/// `isLocked`/`isSilenced`/`isSuspended`/`followersCount`/`followingCount`/`notesCount`
/// を non-nullable 必須フィールドとして要求し、欠けていると（キー不足 or null）
/// 生の Dart `TypeError` を投げる。呼び出し元（Aria の go_router `/miauth` リダイレクト）に
/// 例外処理が無いため、フィールド不足はアプリのフリーズ（画面が真っ黒になる）に直結する
/// （実機で確認済み）。フォロー数等は今回は正確な集計をせず `0`/`false` の安全な既定値で埋める。
#[derive(Serialize)]
pub struct CheckResponseUser {
    pub id: String,
    pub name: Option<String>,
    pub username: String,
    pub host: Option<String>,
    #[serde(rename = "avatarUrl")]
    pub avatar_url: Option<String>,
    #[serde(rename = "isBot")]
    pub is_bot: bool,
    #[serde(rename = "isCat")]
    pub is_cat: bool,
    #[serde(rename = "isLocked")]
    pub is_locked: bool,
    #[serde(rename = "isSilenced")]
    pub is_silenced: bool,
    #[serde(rename = "isSuspended")]
    pub is_suspended: bool,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "followersCount")]
    pub followers_count: i64,
    #[serde(rename = "followingCount")]
    pub following_count: i64,
    #[serde(rename = "notesCount")]
    pub notes_count: i64,
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
            return ApiError::BadRequest("無効なコールバック URL です".to_string()).into_response();
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

    let actor = match state.actors.find_local_by_user_id(auth_user.user_id).await {
        Ok(Some(a)) => a,
        Ok(None) => return ApiError::NotFound("USER_NOT_FOUND").into_response(),
        Err(e) => {
            tracing::error!("[miauth] ユーザー検索失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };

    // third-party クライアントへ渡すアクセストークンは、既存の `Authorization: Bearer`
    // 認証（`extract_auth`/`LocalAuthProvider::verify_token`）がそのまま検証できるよう、
    // 自社ログインと同じ JWT を発行する（トークン検証の経路をもう一本増やさない）。
    // 以前は無意味なダミー文字列（`miauth-token-<uuid>`）を発行していたため、JWT として
    // 検証できず、タイムライン閲覧（未認証で見られる）は動いても投稿等の要認証操作が
    // 401 になっていた。既知の制約: アプリ単位の失効・権限スコープは未対応
    // （自社ログインのトークンと同じ扱いのため）。
    let token = match state.local_auth.generate_token(auth_user.user_id, &auth_user.email) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[miauth] トークン生成失敗: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };
    let mut map = state.miauth_sessions.write().await;
    map.insert(
        session_id,
        MiAuthSession {
            token: Some(token),
            user_id: Some(actor.id),
            username: Some(actor.username),
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
    // 本家 Misskey の access_tokens.fetched と同じく、認可成立後の check は一度きりしか
    // 成功しない（成立済みセッションは取り出すと同時に削除する）。未認可・不明セッションは
    // 常に 200 + {"ok": false} を返す（本家サーバーもここは非2xxを返さない。クライアント
    // （Aria 等）はこのチェックを1回きり・例外未捕捉で呼ぶ実装が多く、非2xxを返すとクライアント
    // 側で未処理例外となり画面がフリーズする — 実機で確認済み）。
    let claimed = {
        let mut map = state.miauth_sessions.write().await;
        let is_ready = matches!(
            map.get(session_id),
            Some(s) if s.token.is_some() && s.user_id.is_some() && s.username.is_some()
        );
        if is_ready { map.remove(session_id) } else { None }
    };

    let Some(session) = claimed else {
        return Json(serde_json::json!({ "ok": false })).into_response();
    };
    // is_ready の判定を通ったので unwrap 安全。
    let token = session.token.unwrap();
    let actor_id = session.user_id.unwrap();
    let username = session.username.unwrap();

    // `misskey_dart` の `UserDetailedNotMe.fromJson` は id/username/isBot/isCat/createdAt/
    // isLocked/isSilenced/isSuspended/followersCount/followingCount/notesCount を
    // non-nullable 必須として要求する（欠けると Dart 側で TypeError → 未処理例外でフリーズ）。
    // フォロー数等は今回正確な集計をせず安全な既定値（0/false）で埋める。
    let row: Option<(chrono::DateTime<chrono::Utc>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT a.created_at, a.display_name, \
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) \
         FROM actors a \
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id \
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
         WHERE a.id = $1",
    )
    .bind(actor_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let (created_at, display_name, avatar_url) = row.unwrap_or_else(|| (chrono::Utc::now(), None, None));

    let res = CheckResponse {
        ok: true,
        token,
        user: CheckResponseUser {
            id: actor_id.to_string(),
            name: display_name,
            username,
            host: None,
            avatar_url,
            is_bot: false,
            is_cat: false,
            is_locked: false,
            is_silenced: false,
            is_suspended: false,
            created_at: created_at.to_rfc3339(),
            followers_count: 0,
            following_count: 0,
            notes_count: 0,
        },
    };
    Json(res).into_response()
}

#[cfg(test)]
mod tests {
    use super::{is_valid_callback, CheckResponseUser};

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

    /// `misskey_dart` の `UserDetailedNotMe.fromJson`（Aria 等が使用）は
    /// id/username/isBot/isCat/createdAt/isLocked/isSilenced/isSuspended/
    /// followersCount/followingCount/notesCount を non-nullable 必須として要求する。
    /// これらのキーが欠けると Dart 側で未処理の TypeError となりアプリがフリーズする
    /// （実機で確認済みの回帰）。JSON 化した結果に全キーが含まれることを固定するテスト。
    #[test]
    fn check_response_user_includes_all_misskey_dart_required_fields() {
        let user = CheckResponseUser {
            id: "1".to_owned(),
            name: Some("表示名".to_owned()),
            username: "alice".to_owned(),
            host: None,
            avatar_url: None,
            is_bot: false,
            is_cat: false,
            is_locked: false,
            is_silenced: false,
            is_suspended: false,
            created_at: "2026-01-01T00:00:00+00:00".to_owned(),
            followers_count: 0,
            following_count: 0,
            notes_count: 0,
        };
        let value = serde_json::to_value(&user).unwrap();
        for key in [
            "id",
            "username",
            "isBot",
            "isCat",
            "createdAt",
            "isLocked",
            "isSilenced",
            "isSuspended",
            "followersCount",
            "followingCount",
            "notesCount",
        ] {
            assert!(
                value.get(key).is_some_and(|v| !v.is_null()),
                "必須フィールド `{key}` が欠けているか null です（Aria 側で TypeError の原因になる）"
            );
        }
    }
}

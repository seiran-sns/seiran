use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::IntoResponse,
    Json,
};

use seiran_common::atp::{resolve_service_endpoint, sign_service_auth_jwt};

use super::{extract_bearer, service_did};
use crate::error::ApiError;
use crate::AppState;

/// 既知のXRPCルートにマッチしなかったリクエストのフォールバック。
///
/// `atproto-proxy` ヘッダー（`<did>#<service-id>` 形式、例:
/// `did:web:api.bsky.app#bsky_appview`）が付いていれば、その宛先へ透過的に転送する。
/// AT Protocolでは `app.bsky.feed.getTimeline`/`searchPosts`等のAppView専用メソッドは
/// PDS自身が実装するのではなく、**PDSがAppViewへの透過プロキシとして振る舞う**ことで
/// 実現される。未実装だとBluesky公式クライアントのタイムライン・検索・通知等が軒並み
/// 「接続できません」になる（2026-08-20 マイケル実機確認）。
///
/// クライアントのaccessJwtをそのまま転送するのではなく、ユーザーの署名鍵
/// （`at_signing_key_pem`）で新たに短命サービス間認証JWTを発行して転送する
/// （`iss`=ユーザーDID、`aud`=`atproto-proxy`ヘッダーの値そのもの、`lxm`=呼び出された
/// XRPCメソッド名。`sign_service_auth_jwt`は`uploadBlob`コールバック検証と共通）。
pub async fn xrpc_proxy_fallback(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let Some(xrpc_method) = uri.path().strip_prefix("/xrpc/") else {
        return ApiError::NotFound("Not Found").into_response();
    };

    let Some(proxy_header) = headers
        .get("atproto-proxy")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "MethodNotImplemented",
                "message": format!("{} は実装されていません", xrpc_method),
            })),
        )
            .into_response();
    };

    let Some((target_did, service_id)) = proxy_header.split_once('#') else {
        return ApiError::BadRequest("不正な atproto-proxy ヘッダーです".to_string())
            .into_response();
    };

    let Some(token) = extract_bearer(&headers) else {
        return ApiError::Unauthorized("Authorization ヘッダーが必要です").into_response();
    };
    let verified = match state
        .local_auth
        .verify_atp_access_token(token, &service_did(&state))
    {
        Ok(v) => v,
        Err(_) => return ApiError::Unauthorized("トークンが無効です").into_response(),
    };
    let actor = match state.actors.find_by_did(&verified.did).await {
        Ok(Some(a)) => a,
        _ => return ApiError::Unauthorized("アクターが見つかりません").into_response(),
    };
    let Some(signing_key_pem) = actor.at_signing_key_pem else {
        return ApiError::Internal("署名鍵が未設定です".to_string()).into_response();
    };

    let target_endpoint =
        match resolve_service_endpoint(target_did, service_id, &state.http_client).await {
            Ok(ep) => ep,
            Err(e) => {
                return ApiError::Internal(format!("プロキシ先解決失敗 ({}): {}", proxy_header, e))
                    .into_response()
            }
        };

    // `aud` クレームはサービスDIDのみ（フラグメント無し）。`atproto-proxy` ヘッダーの
    // `#service-id` 部分はサービスエンドポイント解決にのみ使い、JWTには含めない
    // （フラグメント込みで署名すると対象サービス側で `BadJwtAudience` になる、実機確認）。
    let service_auth_jwt =
        match sign_service_auth_jwt(&signing_key_pem, &verified.did, target_did, xrpc_method) {
            Ok(jwt) => jwt,
            Err(e) => {
                return ApiError::Internal(format!("プロキシ用JWT署名失敗: {}", e)).into_response()
            }
        };

    let target_url = match uri.query() {
        Some(q) => format!("{}/xrpc/{}?{}", target_endpoint, xrpc_method, q),
        None => format!("{}/xrpc/{}", target_endpoint, xrpc_method),
    };

    let mut req = state
        .http_client
        .request(method, &target_url)
        .header("Authorization", format!("Bearer {}", service_auth_jwt));
    if !body.is_empty() {
        let content_type = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json")
            .to_string();
        req = req.header("Content-Type", content_type).body(body);
    }

    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            let content_type = resp.headers().get("content-type").cloned();
            let bytes = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    return ApiError::Internal(format!("プロキシ応答読み取り失敗: {}", e))
                        .into_response()
                }
            };
            let mut builder = axum::http::Response::builder().status(status);
            if let Some(ct) = content_type {
                builder = builder.header("content-type", ct);
            }
            builder
                .body(axum::body::Body::from(bytes))
                .expect("レスポンス構築失敗")
                .into_response()
        }
        Err(e) => {
            ApiError::Internal(format!("プロキシ転送失敗 ({}): {}", target_url, e)).into_response()
        }
    }
}

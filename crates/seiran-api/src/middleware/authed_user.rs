//! 認証必須のユーザー・アクター情報を解決する axum extractor。
//!
//! 以前は全ハンドラが「`extract_auth` で JWT 検証 → `find_local_by_user_id` で
//! アクター行を解決 → 見つからなければ 404」という同じ10行前後を毎回書いていた
//! （一部は `(StatusCode::NOT_FOUND, "アクターが見つかりません")` という生タプルを返し、
//! `ApiError` の JSON 形式と異なる素のテキストボディになっていたため、
//! フロントエンドの `res.json()` がパースに失敗する latent バグでもあった）。
//! この extractor はその定型処理を一本化し、失敗時は必ず `ApiError` の
//! JSON レスポンスを返す。

use axum::{
    extract::FromRequestParts,
    http::{request::Parts, HeaderMap},
    response::{IntoResponse, Response},
};

use crate::{error::ApiError, middleware::extract_auth, AppState};

/// 認証済みユーザー本人 + そのローカルアクター情報。
///
/// ハンドラの引数に `user: AuthedUser` を追加するだけで、Authorization ヘッダー欠如・
/// トークン無効・アクター未解決のいずれも自動的に `ApiError` の JSON レスポンスとして
/// 返るようになる。
#[derive(Debug, Clone)]
pub struct AuthedUser {
    pub user_id: i64,
    pub email: String,
    pub actor_id: i64,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
}

impl AuthedUser {
    /// `HeaderMap` から直接解決する（Misskey 互換ブリッジ等、既に `headers`/`state` を
    /// 手元に持っている非 extractor 経路から呼ぶための共通ロジック）。
    pub async fn from_headers(headers: &HeaderMap, state: &AppState) -> Result<Self, Response> {
        let auth_user = extract_auth(headers, &state.local_auth)
            .await
            .map_err(IntoResponse::into_response)?;
        let actor = state
            .actors
            .find_local_by_user_id(auth_user.user_id)
            .await
            .map_err(|e| ApiError::Internal(format!("アクター取得失敗: {}", e)).into_response())?
            .ok_or_else(|| ApiError::NotFound("ACTOR_NOT_FOUND").into_response())?;

        Ok(AuthedUser {
            user_id: auth_user.user_id,
            email: auth_user.email,
            actor_id: actor.id,
            username: actor.username,
            domain: actor.domain,
            display_name: actor.display_name,
        })
    }
}

#[axum::async_trait]
impl FromRequestParts<AppState> for AuthedUser {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        AuthedUser::from_headers(&parts.headers, state).await
    }
}

/// `AuthedUser` の任意版。未認証・トークン無効・アクター未解決のいずれでもエラーにせず
/// `None` を返す（公開エンドポイントで「ログイン中ならパーソナライズする」用途）。
#[derive(Debug, Clone)]
pub struct MaybeAuthedUser(pub Option<AuthedUser>);

#[axum::async_trait]
impl FromRequestParts<AppState> for MaybeAuthedUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        Ok(MaybeAuthedUser(AuthedUser::from_headers(&parts.headers, state).await.ok()))
    }
}

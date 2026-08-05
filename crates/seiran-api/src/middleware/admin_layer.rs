//! 管理系ルート（`/api/admin/*`）の認可をルータ層で強制するミドルウェア（#221）。
//!
//! 従来は各ハンドラの本体先頭で`require_admin`等を手動で呼ぶ規約になっており、
//! 新規ハンドラ追加時に1行書き忘れると無認可で通ってしまう構造だった
//! （docs/code_audit_2026-08-05.md R-1）。ロールごとに専用ルータを1本ずつ作り、
//! `route_layer`でこのミドルウェアを被せることで、ルート追加時に認可漏れが
//! 起こり得ない構造にする。認可結果の`AuthUser`はリクエストのextensionsへ格納し、
//! 監査ログ等でトークン発行者情報が必要なハンドラは`Extension<AuthUser>`で取り出す。

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::auth::{require_admin, require_emoji_admin, require_report_moderator};
use crate::AppState;

pub async fn admin_only(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    match require_admin(
        req.headers(),
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await
    {
        Ok(user) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(e) => e.into_response(),
    }
}

pub async fn emoji_admin_only(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    match require_emoji_admin(
        req.headers(),
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await
    {
        Ok(user) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(e) => e.into_response(),
    }
}

pub async fn report_moderator_only(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    match require_report_moderator(
        req.headers(),
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await
    {
        Ok(user) => {
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(e) => e.into_response(),
    }
}

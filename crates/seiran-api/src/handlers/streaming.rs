//! リアルタイム更新の WebSocket エンドポイント（#37）。
//!
//! `GET /api/streaming?token=<JWT>` で接続する。ブラウザの WebSocket は
//! Authorization ヘッダを付けられないため、トークンはクエリで受ける。

use std::time::Duration;

use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;

#[derive(Deserialize)]
pub struct StreamQuery {
    pub token: String,
}

pub async fn streaming(
    ws: WebSocketUpgrade,
    Query(q): Query<StreamQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let verified = match state.local_auth.verify_token(&q.token) {
        Ok(v) => v,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid token").into_response(),
    };
    let actor_id = match state.actors.find_local_by_user_id(verified.user_id).await {
        Ok(Some(a)) => a.id,
        _ => return (StatusCode::NOT_FOUND, "actor not found").into_response(),
    };
    ws.on_upgrade(move |socket| handle_stream(socket, actor_id, state))
}

async fn handle_stream(mut socket: WebSocket, actor_id: i64, state: AppState) {
    let mut rx = state.stream_hub.subscribe();
    let mut ping = tokio::time::interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            recv = rx.recv() => match recv {
                Ok(ev) => {
                    if ev.recipients.contains(&actor_id)
                        && socket.send(Message::Text((*ev.payload).clone())).await.is_err()
                    {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => continue, // 取りこぼしは無視（次のフェッチで補完される）
                Err(RecvError::Closed) => break,
            },
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(_)) => break,
                Some(Ok(_)) => {} // クライアントからのメッセージ（pong 等）は無視
            }
        }
    }
}

//! リアルタイム更新の WebSocket エンドポイント（#37）。
//!
//! `GET /api/streaming?token=<JWT>` で接続する。ブラウザの WebSocket は
//! Authorization ヘッダを付けられないため、トークンはクエリで受ける。
//!
//! Misskey互換の`connect`/`channel`/`disconnect`チャンネル購読プロトコルに対応する。
//! クライアントは`{"type":"connect","body":{"channel":"localTimeline","id":"<uuid>","params":{}}}`
//! を送るとそのチャンネル該当のノートを`{"type":"channel","body":{"id":"<uuid>","type":"note","body":{...}}}`
//! で受け取れる。`{"type":"disconnect","body":{"id":"<uuid>"}}`で購読解除する。
//! 通知・DM・`noteUpdated`は従来通り`recipients`方式（購読不要、認証済み接続には自動配信）。

use std::collections::HashMap;
use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::IntoResponse,
};
use seiran_common::streaming::ChannelKind;
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::error::ApiError;
use crate::AppState;

#[derive(Deserialize)]
pub struct StreamQuery {
    pub token: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", content = "body", rename_all = "camelCase")]
enum ClientMessage {
    Connect {
        channel: String,
        id: String,
        #[serde(default)]
        params: serde_json::Value,
    },
    Disconnect {
        id: String,
    },
    #[serde(other)]
    Unknown,
}

pub async fn streaming(
    ws: WebSocketUpgrade,
    Query(q): Query<StreamQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let verified = match state.local_auth.verify_token(&q.token) {
        Ok(v) => v,
        Err(_) => return ApiError::Unauthorized("invalid token").into_response(),
    };
    let actor_id = match state.actors.find_local_by_user_id(verified.user_id).await {
        Ok(Some(a)) => a.id,
        _ => return ApiError::NotFound("actor not found").into_response(),
    };
    ws.on_upgrade(move |socket| handle_stream(socket, actor_id, state))
}

/// `userList`チャンネルの`connect`要求を認可する（`GET /api/lists/:id`と同じ判定:
/// 所有者本人、または公開リストなら誰でも購読できる）。
async fn authorize_user_list(state: &AppState, actor_id: i64, list_id: i64) -> bool {
    match state.lists.find_by_id(list_id).await {
        Ok(Some(row)) => row.is_public || row.owner_actor_id == actor_id,
        _ => false,
    }
}

async fn handle_stream(mut socket: WebSocket, actor_id: i64, state: AppState) {
    let mut rx = state.stream_hub.subscribe();
    let mut ping = tokio::time::interval(Duration::from_secs(30));
    // クライアント指定の subscription id（uuid文字列）-> 購読チャンネル。
    let mut subscriptions: HashMap<String, ChannelKind> = HashMap::new();

    loop {
        tokio::select! {
            recv = rx.recv() => match recv {
                Ok(ev) => {
                    if ev.recipients.contains(&actor_id)
                        && socket.send(Message::Text((*ev.payload).clone())).await.is_err()
                    {
                        break;
                    }
                    if let Some(ref ch) = ev.channel {
                        for (sub_id, kind) in subscriptions.iter() {
                            if ch.scope.matches(kind, actor_id) {
                                let frame = serde_json::json!({
                                    "type": "channel",
                                    "body": { "id": sub_id, "type": "note", "body": ch.note_json },
                                })
                                .to_string();
                                if socket.send(Message::Text(frame)).await.is_err() {
                                    return;
                                }
                            }
                        }
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
                Some(Ok(Message::Text(text))) => {
                    let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) else {
                        continue;
                    };
                    match client_msg {
                        ClientMessage::Connect { channel, id, params } => {
                            let kind = match ChannelKind::parse(&channel, &params) {
                                Some(ChannelKind::UserList(list_id)) => {
                                    if authorize_user_list(&state, actor_id, list_id).await {
                                        Some(ChannelKind::UserList(list_id))
                                    } else {
                                        None
                                    }
                                }
                                other => other,
                            };
                            match kind {
                                Some(k) => {
                                    subscriptions.insert(id, k);
                                }
                                None => {
                                    let err = serde_json::json!({
                                        "type": "error",
                                        "body": { "id": id, "reason": "invalid channel or params" },
                                    })
                                    .to_string();
                                    if socket.send(Message::Text(err)).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                        ClientMessage::Disconnect { id } => {
                            subscriptions.remove(&id);
                        }
                        ClientMessage::Unknown => {}
                    }
                }
                Some(Ok(_)) => {} // pong 等バイナリ/その他フレームは無視
            }
        }
    }
}

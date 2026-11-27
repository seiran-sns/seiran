//! リアルタイム更新の WebSocket エンドポイント（#37）。
//!
//! `GET /api/streaming?token=<JWT>` で接続する。ブラウザの WebSocket は
//! Authorization ヘッダを付けられないため、トークンはクエリで受ける。
//!
//! Misskey互換の`connect`/`channel`/`disconnect`チャンネル購読プロトコルに対応する。
//! クライアントは`{"type":"connect","body":{"channel":"localTimeline","id":"<uuid>","params":{}}}`
//! を送るとそのチャンネル該当のノートを`{"type":"channel","body":{"id":"<uuid>","type":"note","body":{...}}}`
//! で受け取れる。`{"type":"disconnect","body":{"id":"<uuid>"}}`で購読解除する。
//! 通知・DMは`recipients`方式（購読不要、認証済み接続には自動配信）。
//! リアクションの増減（`noteUpdated`）とアンケートの票数（`pollUpdated`）は、`{"type":"subNote","body":{"id":"<noteId>"}}`で
//! 購読したノートにも届く（本家 Misskey の`subNote`/`unsubNote`と同じ。画面に出している
//! ノートを購読すれば、投稿者をフォローしていなくても更新が届く。`streaming::NoteSubscribers`）。

use std::collections::HashMap;
use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::IntoResponse,
};
use seiran_common::streaming::{ChannelBroadcast, ChannelKind, NoteSubscriptions};
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
    SubNote {
        id: String,
    },
    UnsubNote {
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
    let verified = match state.local_auth.verify_token_ignoring_exp(&q.token) {
        Ok(v) => v,
        Err(_) => return ApiError::Unauthorized("invalid token").into_response(),
    };
    let actor_id = match state.actors.find_local_by_user_id(verified.user_id).await {
        Ok(Some(a)) => a.id,
        _ => return ApiError::NotFound("actor not found").into_response(),
    };
    let user_id = verified.user_id;
    ws.on_upgrade(move |socket| handle_stream(socket, actor_id, user_id, state))
}

/// `userList`チャンネルの`connect`要求を認可する（`GET /api/lists/:id`と同じ判定:
/// 所有者本人、または公開リストなら誰でも購読できる）。
async fn authorize_user_list(state: &AppState, actor_id: i64, list_id: i64) -> bool {
    match state.lists.find_by_id(list_id).await {
        Ok(Some(row)) => row.is_public || row.owner_actor_id == actor_id,
        _ => false,
    }
}

/// モデレーション表示のチャンネルで流すノートのうち、購読者（管理者）自身からは本来見えない
/// ものに`visibleByAdminOverride`を付けたJSONを返す（付けなくてよければ`None`）。対象は
/// プライベート（`followers_only`）だけで、著者・フォロワー（`home_recipients`）は確実に見える
/// のでDBに問い合わせない。フロントはこのフラグでグレー表示・操作不可にする。
async fn moderation_override_body(
    state: &AppState,
    ch: &ChannelBroadcast,
    actor_id: i64,
) -> Option<serde_json::Value> {
    if ch.scope.visibility != "followers_only" || ch.scope.home_recipients.contains(&actor_id) {
        return None;
    }
    let post_id: i64 = ch.note_json.get("id")?.as_str()?.parse().ok()?;
    let visible =
        seiran_common::repository::find_visible_posts_by_ids(&state.db, &[post_id], Some(actor_id))
            .await
            .map(|rows| !rows.is_empty())
            .unwrap_or(false);
    if visible {
        return None;
    }
    let mut body = (*ch.note_json).clone();
    body["visibleByAdminOverride"] = serde_json::Value::Bool(true);
    Some(body)
}

/// サーバーの停止（無停止更新で旧プロセスを止めるとき）に送る Close フレーム。
/// 1012（Service Restart）を受けたクライアントはすぐ再接続してよく、新しいプロセスにつながる。
pub(crate) fn restart_close_message() -> Message {
    Message::Close(Some(axum::extract::ws::CloseFrame {
        code: axum::extract::ws::close_code::RESTART,
        reason: "server restart".into(),
    }))
}

async fn handle_stream(mut socket: WebSocket, actor_id: i64, user_id: i64, state: AppState) {
    let mut rx = state.stream_hub.subscribe();
    let mut ping = tokio::time::interval(Duration::from_secs(30));
    // クライアント指定の subscription id（uuid文字列）-> 購読チャンネル。
    let mut subscriptions: HashMap<String, ChannelKind> = HashMap::new();
    let mut note_subscriptions = NoteSubscriptions::default();

    loop {
        tokio::select! {
            _ = state.shutdown.cancelled() => {
                let _ = socket.send(restart_close_message()).await;
                break;
            }
            recv = rx.recv() => match recv {
                Ok(ev) => {
                    if (ev.broadcast_all
                        || ev.recipients.contains(&actor_id)
                        || note_subscriptions.admits(&ev, Some(actor_id)))
                        && socket.send(Message::Text((*ev.payload).clone())).await.is_err()
                    {
                        break;
                    }
                    if let Some(ref ch) = ev.channel {
                        for (sub_id, kind) in subscriptions.iter() {
                            if ch.scope.matches(kind, actor_id) {
                                let override_body = if *kind == ChannelKind::LocalTimelineModeration {
                                    moderation_override_body(&state, ch, actor_id).await
                                } else {
                                    None
                                };
                                let body = override_body.as_ref().unwrap_or(&ch.note_json);
                                let frame = serde_json::json!({
                                    "type": "channel",
                                    "body": { "id": sub_id, "type": "note", "body": body },
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
                                Some(ChannelKind::LocalTimelineModeration) => {
                                    let is_admin = crate::handlers::notes::queries::user_is_admin(
                                        &state, user_id,
                                    )
                                    .await
                                    .unwrap_or(false);
                                    is_admin.then_some(ChannelKind::LocalTimelineModeration)
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
                        ClientMessage::SubNote { id } => note_subscriptions.subscribe(&id),
                        ClientMessage::UnsubNote { id } => note_subscriptions.unsubscribe(&id),
                        ClientMessage::Unknown => {}
                    }
                }
                Some(Ok(_)) => {} // pong 等バイナリ/その他フレームは無視
            }
        }
    }
}

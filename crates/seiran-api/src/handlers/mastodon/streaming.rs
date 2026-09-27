//! Mastodon 互換ストリーミング（`GET /api/v1/streaming`、WebSocket）。
//!
//! 配信元は SPA・Misskey 互換と同じ `StreamHub`。タイムライン新着はチャンネル方式
//! （`ChannelScope::matches`）で購読ストリームに当てはめ、閲覧者視点の `Status` に組み直して
//! `update` イベントで送る。通知は `StreamHub` の自分宛てイベントを「新着がありそう」という
//! 合図として使い、通知テーブルの新着を読んで `notification` イベントで送る（配信イベントは
//! 通知 ID を持たず、通知の INSERT より先に送られることもあるため、少し待ってから読む。
//! 取りこぼしは定期確認で拾う）。
//!
//! ストリーム: `user`（ホーム＋通知）・`user:notification`・`public`・`public:remote`
//! （どちらもグローバル）・`public:local`・`hashtag`（`tag`）・`list`（`list`）。
//! 投稿削除（`delete`）・DM（`direct`）・編集（`status.update`）のイベントは送らない。

use std::collections::HashMap;
use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use seiran_common::streaming::ChannelKind;
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::error::ApiError;
use crate::middleware::extract_auth;
use crate::AppState;

use super::convert::{build_notifications, find_status};
use super::extract::MastodonQuery;

/// 自分宛てイベントを受けてから通知テーブルを読むまでの待ち時間（配信が INSERT より先に
/// 来る経路があるため）。
const NOTIFICATION_SETTLE: Duration = Duration::from_millis(800);
/// 合図を取りこぼした通知を拾う定期確認・Ping の間隔。
const PERIODIC: Duration = Duration::from_secs(30);

/// 購読中のストリーム。`name` はイベントの `stream` 配列（`["hashtag", "foo"]` 等）。
#[derive(Clone, PartialEq, Eq)]
struct Stream {
    name: Vec<String>,
    /// タイムライン新着の判定に使うチャンネル（`user:notification` は `None`）。
    channel: Option<ChannelKind>,
    notifications: bool,
}

/// Mastodon のストリーム指定を解釈する。`list` の認可は呼び出し側で行う。
fn parse_stream(stream: &str, tag: Option<&str>, list: Option<&str>) -> Option<Stream> {
    let simple = |channel: ChannelKind, notifications: bool| Stream {
        name: vec![stream.to_owned()],
        channel: Some(channel),
        notifications,
    };
    Some(match stream {
        "user" => simple(ChannelKind::HomeTimeline, true),
        "user:notification" => Stream {
            name: vec![stream.to_owned()],
            channel: None,
            notifications: true,
        },
        "public" | "public:remote" | "public:media" => simple(ChannelKind::GlobalTimeline, false),
        "public:local" | "public:local:media" => simple(ChannelKind::LocalTimeline, false),
        "hashtag" | "hashtag:local" => {
            let tag = tag?.trim().trim_start_matches('#').to_lowercase();
            if tag.is_empty() {
                return None;
            }
            Stream {
                name: vec![stream.to_owned(), tag.clone()],
                channel: Some(ChannelKind::Hashtag(tag)),
                notifications: false,
            }
        }
        "list" => {
            let id = list?.parse::<i64>().ok()?;
            Stream {
                name: vec![stream.to_owned(), id.to_string()],
                channel: Some(ChannelKind::UserList(id)),
                notifications: false,
            }
        }
        _ => return None,
    })
}

#[derive(Deserialize, Default)]
pub struct StreamingQuery {
    pub access_token: Option<String>,
    pub stream: Option<String>,
    pub tag: Option<String>,
    pub list: Option<String>,
}

#[derive(Deserialize)]
struct ClientMessage {
    #[serde(rename = "type")]
    kind: String,
    stream: String,
    tag: Option<String>,
    list: Option<String>,
}

/// GET /api/v1/streaming/health
pub async fn health() -> &'static str {
    "OK"
}

/// GET /api/v1/streaming — トークンはクエリ `access_token`、`Authorization` ヘッダー、
/// `Sec-WebSocket-Protocol`（ブラウザの WebSocket はヘッダーを付けられないため、一部の
/// クライアントはここにトークンを入れる。その場合は同じ値をプロトコルとして応答に返す）の順に探す。
pub async fn streaming(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<AppState>,
    MastodonQuery(q): MastodonQuery<StreamingQuery>,
) -> Response {
    let protocol_token = headers
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty());
    let token = q
        .access_token
        .clone()
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(str::to_owned)
        })
        .or_else(|| protocol_token.clone());
    let Some(token) = token else {
        return ApiError::Unauthorized("Missing access token").into_response();
    };
    let mut auth_headers = HeaderMap::new();
    let Ok(value) = format!("Bearer {token}").parse() else {
        return ApiError::Unauthorized("Invalid access token").into_response();
    };
    auth_headers.insert("authorization", value);
    let user = match extract_auth(
        &auth_headers,
        &state.local_auth,
        state.app_tokens.as_ref(),
        state.users.as_ref(),
    )
    .await
    {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    let actor_id = match state.actors.find_local_by_user_id(user.user_id).await {
        Ok(Some(a)) => a.id,
        Ok(None) => return ApiError::NotFound("ACTOR_NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(e.to_string()).into_response(),
    };

    let mut initial = Vec::new();
    if let Some(stream) = q.stream.as_deref() {
        match parse_stream(stream, q.tag.as_deref(), q.list.as_deref()) {
            Some(s) if authorize(&state, actor_id, &s).await => initial.push(s),
            _ => return ApiError::BadRequest("Unknown stream type".to_owned()).into_response(),
        }
    }

    let ws = match (protocol_token, q.access_token.is_none()) {
        (Some(p), true) => ws.protocols([p]),
        _ => ws,
    };
    ws.on_upgrade(move |socket| handle(socket, state, actor_id, initial))
}

/// `list` は所有者か公開リストのみ（REST の `timelines/list` と同じ判定）。
async fn authorize(state: &AppState, actor_id: i64, stream: &Stream) -> bool {
    match stream.channel {
        Some(ChannelKind::UserList(list_id)) => matches!(
            state.lists.find_by_id(list_id).await,
            Ok(Some(row)) if row.is_public || row.owner_actor_id == actor_id
        ),
        _ => true,
    }
}

fn frame(stream: &[String], event: &str, payload: &impl serde::Serialize) -> Option<Message> {
    let payload = serde_json::to_string(payload).ok()?;
    let text =
        serde_json::json!({ "stream": stream, "event": event, "payload": payload }).to_string();
    Some(Message::Text(text))
}

/// 1接続の購読状態。
struct Connection {
    state: AppState,
    actor_id: i64,
    /// 購読キー（`stream` 配列を連結したもの）→ ストリーム。
    streams: HashMap<String, Stream>,
    /// 送信済みの最新通知 ID（接続時点の最新から数える）。
    last_notification_id: Option<i64>,
}

impl Connection {
    fn wants_notifications(&self) -> bool {
        self.streams.values().any(|s| s.notifications)
    }

    async fn update_frames(
        &self,
        note_json: &serde_json::Value,
        scope_matches: impl Fn(&ChannelKind) -> bool,
    ) -> Vec<Message> {
        let names: Vec<&Vec<String>> = self
            .streams
            .values()
            .filter(|s| s.channel.as_ref().is_some_and(&scope_matches))
            .map(|s| &s.name)
            .collect();
        if names.is_empty() {
            return Vec::new();
        }
        let Some(post_id) = note_json["id"].as_str().and_then(|s| s.parse::<i64>().ok()) else {
            return Vec::new();
        };
        // 閲覧者に見えない投稿（ブロック・可視性）は `find_status` が弾く。
        let Ok(status) = find_status(&self.state, post_id, Some(self.actor_id)).await else {
            return Vec::new();
        };
        names
            .into_iter()
            .filter_map(|name| frame(name, "update", &status))
            .collect()
    }

    async fn notification_frames(&mut self) -> Vec<Message> {
        if !self.wants_notifications() {
            return Vec::new();
        }
        let Ok(rows) = self
            .state
            .notifications
            .list(self.actor_id, 20, None, self.last_notification_id)
            .await
        else {
            return Vec::new();
        };
        let Some(newest) = rows.first().map(|r| r.id) else {
            return Vec::new();
        };
        let first_check = self.last_notification_id.is_none();
        self.last_notification_id = Some(newest);
        // 接続直後の確認は基準点を決めるだけ（過去の通知は REST で取る）。
        if first_check {
            return Vec::new();
        }
        let Ok(mut notifications) = build_notifications(&self.state, rows, self.actor_id).await
        else {
            return Vec::new();
        };
        notifications.reverse();
        let names: Vec<Vec<String>> = self
            .streams
            .values()
            .filter(|s| s.notifications)
            .map(|s| s.name.clone())
            .collect();
        notifications
            .iter()
            .flat_map(|n| {
                names
                    .iter()
                    .filter_map(move |name| frame(name, "notification", n))
            })
            .collect()
    }

    async fn apply_client_message(&mut self, text: &str) {
        let Ok(msg) = serde_json::from_str::<ClientMessage>(text) else {
            return;
        };
        let Some(stream) = parse_stream(&msg.stream, msg.tag.as_deref(), msg.list.as_deref())
        else {
            return;
        };
        let key = stream.name.join(":");
        match msg.kind.as_str() {
            "subscribe" if authorize(&self.state, self.actor_id, &stream).await => {
                self.streams.insert(key, stream);
            }
            "unsubscribe" => {
                self.streams.remove(&key);
            }
            _ => {}
        }
    }
}

async fn send_all(socket: &mut WebSocket, frames: Vec<Message>) -> bool {
    for f in frames {
        if socket.send(f).await.is_err() {
            return false;
        }
    }
    true
}

async fn handle(mut socket: WebSocket, state: AppState, actor_id: i64, initial: Vec<Stream>) {
    let mut rx = state.stream_hub.subscribe();
    let mut conn = Connection {
        state,
        actor_id,
        streams: initial.into_iter().map(|s| (s.name.join(":"), s)).collect(),
        last_notification_id: None,
    };
    // 通知の基準点（接続時点の最新）を決めておく。
    conn.notification_frames().await;
    if conn.last_notification_id.is_none() {
        conn.last_notification_id = Some(0);
    }

    let mut periodic = tokio::time::interval(PERIODIC);
    let settle = tokio::time::sleep(Duration::from_secs(u64::MAX / 4));
    tokio::pin!(settle);
    let mut settle_armed = false;

    loop {
        tokio::select! {
            recv = rx.recv() => match recv {
                Ok(ev) => {
                    if ev.recipients.contains(&actor_id) && !settle_armed && conn.wants_notifications() {
                        settle.as_mut().reset(tokio::time::Instant::now() + NOTIFICATION_SETTLE);
                        settle_armed = true;
                    }
                    if let Some(ch) = ev.channel {
                        let frames = conn
                            .update_frames(&ch.note_json, |kind| ch.scope.matches(kind, actor_id))
                            .await;
                        if !send_all(&mut socket, frames).await {
                            break;
                        }
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            },
            _ = &mut settle, if settle_armed => {
                settle_armed = false;
                let frames = conn.notification_frames().await;
                if !send_all(&mut socket, frames).await {
                    break;
                }
            }
            _ = periodic.tick() => {
                let frames = conn.notification_frames().await;
                if !send_all(&mut socket, frames).await || socket.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(text))) => conn.apply_client_message(&text).await,
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mastodon_stream_names() {
        let user = parse_stream("user", None, None).unwrap();
        assert_eq!(user.channel, Some(ChannelKind::HomeTimeline));
        assert!(user.notifications);
        assert_eq!(
            parse_stream("public:local", None, None).unwrap().channel,
            Some(ChannelKind::LocalTimeline)
        );
        let tag = parse_stream("hashtag", Some("#Rust"), None).unwrap();
        assert_eq!(tag.name, vec!["hashtag", "rust"]);
        assert_eq!(tag.channel, Some(ChannelKind::Hashtag("rust".into())));
        assert_eq!(
            parse_stream("list", None, Some("12")).unwrap().name,
            vec!["list", "12"]
        );
        assert!(parse_stream("hashtag", None, None).is_none());
        assert!(parse_stream("direct", None, None).is_none());
    }
}

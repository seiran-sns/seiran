//! Misskey 互換ストリーミング（`GET /streaming`、WebSocket）。
//!
//! Misskey クライアント（Aria・Miria 等）は `wss://<host>/streaming?i=<token>` に接続し、
//! 本家 `packages/backend/src/server/api/stream/Connection.ts` と同じメッセージをやり取りする。
//! トークンは `Authorization: Bearer` かクエリ `i`（`middleware::misskey_auth_bridge` が
//! ヘッダーへ合成する）。トークン無しの接続は本家と同じく匿名で受け付け、ログイン不要の
//! チャンネル（`localTimeline`・`globalTimeline`・`hashtag`）だけ購読できる。
//!
//! 配信元は SPA・Mastodon 互換と同じ `StreamHub`。SPA 向けの `/api/streaming` とは別にしている
//! のは、配るノート・イベントの形が違うため（SPA のノート JSON をそのまま流すと、Misskey
//! クライアントは `Note` として読めない）。
//!
//! - タイムライン（`connect` の `homeTimeline`・`localTimeline`・`hybridTimeline`・
//!   `globalTimeline`・`userList`・`hashtag`）: `ChannelScope::matches` で当てはめ、閲覧者視点の
//!   Misskey `Note` に組み直して `channel`/`note` で送る。`params.withRenotes: false`・
//!   `params.withFiles: true` は本家と同じく接続ごとに絞り込む。
//! - `main`: 通知を `notification` で送る（メンション・返信の通知は `mention`・`reply` も送る）。
//!   自分宛てイベントを「新着がありそう」という合図にして通知テーブルを読む（配信イベントは
//!   通知 ID を持たず、通知の INSERT より先に送られることもあるため、少し待ってから読む。
//!   取りこぼしは定期確認で拾う）。
//! - `subNote`（`s`・`sr`）で購読したノートには、リアクションの増減を `noteUpdated` の
//!   `reacted`/`unreacted` で送る。元は自分宛ての `noteUpdated`（配信先は投稿者とそのフォロワー、
//!   DM は当事者）なので、それ以外の人のノートの変化は届かない。
//!
//! 未対応のチャンネル名は本家と同じく黙って無視する（エラーを返すとクライアントが接続を
//! 失敗扱いにすることがある）。

use std::collections::HashMap;
use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header::AUTHORIZATION, HeaderMap},
    response::{IntoResponse, Response},
};
use seiran_common::repository::NotificationFilter;
use seiran_common::streaming::{ChannelKind, NoteUpdatedBody, NOTE_UPDATED_EVENT};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::middleware::AuthedUser;
use crate::AppState;

use super::convert::{build_note, build_notifications, is_pure_renote};

/// 自分宛てイベントを受けてから通知テーブルを読むまでの待ち時間（配信が INSERT より先に
/// 来る経路があるため）。
const NOTIFICATION_SETTLE: Duration = Duration::from_millis(800);
/// 合図を取りこぼした通知を拾う定期確認・Ping の間隔。
const PERIODIC: Duration = Duration::from_secs(30);
/// 1接続で購読できるチャンネル数の上限（本家 `MAX_CHANNELS_PER_CONNECTION`）。
const MAX_CHANNELS: usize = 32;

/// 購読中のタイムラインチャンネル。
struct TimelineSub {
    kind: ChannelKind,
    with_renotes: bool,
    with_files: bool,
}

impl TimelineSub {
    fn accepts(&self, note: &super::types::MisskeyNote) -> bool {
        (self.with_renotes || !is_pure_renote(note)) && (!self.with_files || !note.files.is_empty())
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", content = "body", rename_all = "camelCase")]
enum ClientMessage {
    Connect {
        channel: String,
        id: String,
        #[serde(default)]
        params: serde_json::Value,
        #[serde(default)]
        pong: Option<bool>,
    },
    Disconnect {
        id: String,
    },
    #[serde(alias = "s", alias = "sr")]
    SubNote {
        id: String,
    },
    #[serde(alias = "un")]
    UnsubNote {
        id: String,
    },
    #[serde(other)]
    Unknown,
}

/// `{"type": <kind>, "body": <body>}` のテキストフレーム。
fn frame(kind: &str, body: serde_json::Value) -> Message {
    Message::Text(serde_json::json!({ "type": kind, "body": body }).to_string())
}

/// チャンネル `id` 宛てのイベント（`{"type":"channel","body":{"id","type","body"}}`）。
fn channel_frame(id: &str, kind: &str, body: serde_json::Value) -> Message {
    frame(
        "channel",
        serde_json::json!({ "id": id, "type": kind, "body": body }),
    )
}

/// GET /streaming
pub async fn streaming(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    // トークンが付いていて無効なら本家と同じく 401（匿名扱いにして黙って続けない）。
    let viewer = if headers.contains_key(AUTHORIZATION) {
        match AuthedUser::from_headers(&headers, &state).await {
            Ok(u) => Some(u.actor_id),
            Err(resp) => return resp,
        }
    } else {
        None
    };
    ws.on_upgrade(move |socket| handle(socket, state, viewer))
        .into_response()
}

/// 1接続の購読状態。
struct Connection {
    state: AppState,
    /// 閲覧者のアクター ID（匿名接続は `None`）。
    viewer: Option<i64>,
    /// クライアント指定のチャンネル接続 ID → タイムラインチャンネル。
    timelines: HashMap<String, TimelineSub>,
    /// `main` チャンネルの接続 ID。
    main_ids: Vec<String>,
    /// `subNote` で購読中のノート ID → 購読数（本家と同じく `unsubNote` と対で数える）。
    sub_notes: HashMap<String, u32>,
    /// 送信済みの最新通知 ID（`main` 接続時点の最新から数える）。
    last_notification_id: Option<i64>,
}

impl Connection {
    fn channel_count(&self) -> usize {
        self.timelines.len() + self.main_ids.len()
    }

    async fn connect(
        &mut self,
        channel: &str,
        id: String,
        params: &serde_json::Value,
        pong: bool,
    ) -> Vec<Message> {
        if self.channel_count() >= MAX_CHANNELS
            || self.timelines.contains_key(&id)
            || self.main_ids.contains(&id)
        {
            return Vec::new();
        }
        if channel == "main" {
            if self.viewer.is_none() {
                return Vec::new();
            }
            if self.main_ids.is_empty() {
                // 通知の基準点（接続時点の最新）を決めておく。
                self.last_notification_id = None;
                self.main_ids.push(id.clone());
                self.notification_frames().await;
                self.last_notification_id.get_or_insert(0);
            } else {
                self.main_ids.push(id.clone());
            }
        } else {
            let Some(kind) = ChannelKind::parse(channel, params) else {
                return Vec::new();
            };
            if !self.authorize(&kind).await {
                return Vec::new();
            }
            let sub = TimelineSub {
                kind,
                with_renotes: params["withRenotes"].as_bool().unwrap_or(true),
                with_files: params["withFiles"].as_bool().unwrap_or(false),
            };
            self.timelines.insert(id.clone(), sub);
        }
        if pong {
            vec![frame("connected", serde_json::json!({ "id": id }))]
        } else {
            Vec::new()
        }
    }

    /// 閲覧者に依るチャンネルはログインが要る。`userList` は所有者か公開リストのみ（REST の
    /// `notes/user-list-timeline` と同じ判定）。モデレーション表示は SPA 専用。
    async fn authorize(&self, kind: &ChannelKind) -> bool {
        match (kind, self.viewer) {
            (ChannelKind::LocalTimelineModeration, _) => false,
            (
                ChannelKind::LocalTimeline | ChannelKind::GlobalTimeline | ChannelKind::Hashtag(_),
                _,
            ) => true,
            (ChannelKind::UserList(list_id), Some(actor_id)) => matches!(
                self.state.lists.find_by_id(*list_id).await,
                Ok(Some(row)) if row.is_public || row.owner_actor_id == actor_id
            ),
            (ChannelKind::HomeTimeline | ChannelKind::HybridTimeline, Some(_)) => true,
            (_, None) => false,
        }
    }

    fn disconnect(&mut self, id: &str) {
        self.timelines.remove(id);
        self.main_ids.retain(|m| m != id);
    }

    fn sub_note(&mut self, id: String) {
        *self.sub_notes.entry(id).or_insert(0) += 1;
    }

    fn unsub_note(&mut self, id: &str) {
        if let Some(count) = self.sub_notes.get_mut(id) {
            *count -= 1;
            if *count == 0 {
                self.sub_notes.remove(id);
            }
        }
    }

    async fn apply_client_message(&mut self, text: &str) -> Vec<Message> {
        let Ok(msg) = serde_json::from_str::<ClientMessage>(text) else {
            return Vec::new();
        };
        match msg {
            ClientMessage::Connect {
                channel,
                id,
                params,
                pong,
            } => {
                self.connect(&channel, id, &params, pong.unwrap_or(false))
                    .await
            }
            ClientMessage::Disconnect { id } => {
                self.disconnect(&id);
                Vec::new()
            }
            ClientMessage::SubNote { id } => {
                self.sub_note(id);
                Vec::new()
            }
            ClientMessage::UnsubNote { id } => {
                self.unsub_note(&id);
                Vec::new()
            }
            ClientMessage::Unknown => Vec::new(),
        }
    }

    /// タイムラインの新着ノートを、当てはまる購読チャンネルへ閲覧者視点の `Note` で送る。
    async fn note_frames(
        &self,
        note_json: &serde_json::Value,
        scope_matches: impl Fn(&ChannelKind) -> bool,
    ) -> Vec<Message> {
        let ids: Vec<(&String, &TimelineSub)> = self
            .timelines
            .iter()
            .filter(|(_, sub)| scope_matches(&sub.kind))
            .collect();
        if ids.is_empty() {
            return Vec::new();
        }
        let Some(post_id) = note_json["id"].as_str().and_then(|s| s.parse::<i64>().ok()) else {
            return Vec::new();
        };
        // 閲覧者に見えない投稿（ブロック・可視性）は `find_by_id_for_viewer` が弾く。
        let Ok(Some(post)) = self
            .state
            .posts
            .find_by_id_for_viewer(post_id, self.viewer)
            .await
        else {
            return Vec::new();
        };
        let note = build_note(&self.state, post, self.viewer).await;
        let Ok(body) = serde_json::to_value(&note) else {
            return Vec::new();
        };
        ids.into_iter()
            .filter(|(_, sub)| sub.accepts(&note))
            .map(|(id, _)| channel_frame(id, "note", body.clone()))
            .collect()
    }

    /// `main` へ新着通知を送る。
    async fn notification_frames(&mut self) -> Vec<Message> {
        let Some(actor_id) = self.viewer else {
            return Vec::new();
        };
        if self.main_ids.is_empty() {
            return Vec::new();
        }
        let Ok(rows) = self
            .state
            .notifications
            .list(
                actor_id,
                20,
                None,
                self.last_notification_id,
                &NotificationFilter::default(),
            )
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
        let mut notifications = build_notifications(&self.state, rows, actor_id).await;
        notifications.reverse();
        let mut frames = Vec::new();
        for n in notifications {
            // 本家はメンション・返信を通知とは別に `mention`・`reply` でも送り、クライアントは
            // これでメンション欄を更新する。
            let extra = match (n.kind.as_str(), &n.note) {
                ("mention", Some(note)) => Some(("mention", serde_json::to_value(note))),
                ("reply", Some(note)) => Some(("reply", serde_json::to_value(note))),
                _ => None,
            };
            let Ok(body) = serde_json::to_value(&n) else {
                continue;
            };
            for id in &self.main_ids {
                frames.push(channel_frame(id, "notification", body.clone()));
                if let Some((kind, Ok(note))) = &extra {
                    frames.push(channel_frame(id, kind, note.clone()));
                }
            }
        }
        frames
    }

    /// 自分宛ての `noteUpdated` を、購読中のノートなら本家の `reacted`/`unreacted` にする。
    fn reaction_frames(&self, payload: &str) -> Vec<Message> {
        #[derive(Deserialize)]
        struct Envelope {
            #[serde(rename = "type")]
            kind: String,
            body: NoteUpdatedBody,
        }
        // 他の種類のイベントまで毎回 JSON を読まないよう、先に名前で見分ける。
        if self.sub_notes.is_empty() || !payload.contains(NOTE_UPDATED_EVENT) {
            return Vec::new();
        }
        let Ok(envelope) = serde_json::from_str::<Envelope>(payload) else {
            return Vec::new();
        };
        if envelope.kind != NOTE_UPDATED_EVENT
            || !self.sub_notes.contains_key(&envelope.body.post_id)
        {
            return Vec::new();
        }
        note_updated_events(&envelope.body)
            .into_iter()
            .map(|(kind, body)| {
                frame(
                    "noteUpdated",
                    serde_json::json!({ "id": envelope.body.post_id, "type": kind, "body": body }),
                )
            })
            .collect()
    }
}

/// seiran の `noteUpdated` を本家の `reacted`/`unreacted` の列にする。リアクションの付け替えは
/// 外してから付ける。誰の操作か分からない更新（ブリッジ先の兄弟ポストの件数同期）は送らない。
fn note_updated_events(body: &NoteUpdatedBody) -> Vec<(&'static str, serde_json::Value)> {
    let Some(user_id) = body.reactor_actor_id.as_deref() else {
        return Vec::new();
    };
    let mut events = Vec::new();
    if let Some(prev) = body.reactor_previous_emoji.as_deref() {
        events.push((
            "unreacted",
            serde_json::json!({ "reaction": prev, "userId": user_id }),
        ));
    }
    if let Some(reaction) = body.reactor_emoji.as_deref() {
        // カスタム絵文字は本家と同じくコロンを除いた名前と画像 URL を添える（`reactionEmojis` の
        // キーと同じ形）。
        let emoji = body
            .reactions
            .iter()
            .find(|r| r.emoji == reaction)
            .and_then(|r| r.emoji_url.as_deref())
            .map(|url| {
                serde_json::json!({
                    "name": reaction.trim_start_matches(':').trim_end_matches(':'),
                    "url": url,
                })
            });
        events.push((
            "reacted",
            serde_json::json!({ "reaction": reaction, "emoji": emoji, "userId": user_id }),
        ));
    }
    events
}

async fn send_all(socket: &mut WebSocket, frames: Vec<Message>) -> bool {
    for f in frames {
        if socket.send(f).await.is_err() {
            return false;
        }
    }
    true
}

async fn handle(mut socket: WebSocket, state: AppState, viewer: Option<i64>) {
    let mut rx = state.stream_hub.subscribe();
    let shutdown = state.shutdown.clone();
    let mut conn = Connection {
        state,
        viewer,
        timelines: HashMap::new(),
        main_ids: Vec::new(),
        sub_notes: HashMap::new(),
        last_notification_id: None,
    };

    let mut periodic = tokio::time::interval(PERIODIC);
    let settle = tokio::time::sleep(Duration::from_secs(u64::MAX / 4));
    tokio::pin!(settle);
    let mut settle_armed = false;

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                let _ = socket.send(crate::handlers::streaming::restart_close_message()).await;
                break;
            }
            recv = rx.recv() => match recv {
                Ok(ev) => {
                    if viewer.is_some_and(|v| ev.recipients.contains(&v)) {
                        if !settle_armed && !conn.main_ids.is_empty() {
                            settle.as_mut().reset(tokio::time::Instant::now() + NOTIFICATION_SETTLE);
                            settle_armed = true;
                        }
                        let frames = conn.reaction_frames(&ev.payload);
                        if !send_all(&mut socket, frames).await {
                            break;
                        }
                    }
                    if let Some(ch) = ev.channel {
                        // 匿名接続が購読できるのは閲覧者に依らないチャンネルだけ（`authorize`）
                        // なので、閲覧者 ID の代わりの値は判定に使われない。
                        let viewer_id = viewer.unwrap_or_default();
                        let frames = conn
                            .note_frames(&ch.note_json, |kind| ch.scope.matches(kind, viewer_id))
                            .await;
                        if !send_all(&mut socket, frames).await {
                            break;
                        }
                    }
                }
                Err(RecvError::Lagged(_)) => continue, // 取りこぼしは無視（次のフェッチで補完される）
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
                Some(Ok(Message::Text(text))) => {
                    let frames = conn.apply_client_message(&text).await;
                    if !send_all(&mut socket, frames).await {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seiran_common::streaming::ReactionCount;

    #[test]
    fn parses_misskey_client_messages() {
        let connect: ClientMessage = serde_json::from_str(
            r#"{"type":"connect","body":{"channel":"homeTimeline","id":"a","params":{"withRenotes":false},"pong":true}}"#,
        )
        .unwrap();
        assert!(matches!(
            connect,
            ClientMessage::Connect { ref channel, pong: Some(true), .. } if channel == "homeTimeline"
        ));
        for alias in ["s", "sr", "subNote"] {
            let msg: ClientMessage =
                serde_json::from_str(&format!(r#"{{"type":"{alias}","body":{{"id":"1"}}}}"#))
                    .unwrap();
            assert!(matches!(msg, ClientMessage::SubNote { .. }), "{alias}");
        }
        let un: ClientMessage = serde_json::from_str(r#"{"type":"un","body":{"id":"1"}}"#).unwrap();
        assert!(matches!(un, ClientMessage::UnsubNote { .. }));
        // 未対応の種類は（本文付きだと解析エラーになるが）どちらにしても何もしない。
        let other =
            serde_json::from_str::<ClientMessage>(r#"{"type":"readNotification","body":{}}"#);
        assert!(other.map_or(true, |m| matches!(m, ClientMessage::Unknown)));
    }

    fn body(emoji: Option<&str>, previous: Option<&str>) -> NoteUpdatedBody {
        NoteUpdatedBody {
            post_id: "10".into(),
            reactions: vec![ReactionCount {
                emoji: ":blob@example.com:".into(),
                count: 1,
                emoji_url: Some("https://example.com/blob.png".into()),
            }],
            reactor_actor_id: Some("5".into()),
            reactor_emoji: emoji.map(str::to_owned),
            reactor_previous_emoji: previous.map(str::to_owned),
        }
    }

    #[test]
    fn maps_reaction_changes_to_reacted_and_unreacted() {
        let events = note_updated_events(&body(Some(":blob@example.com:"), Some("👍")));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "unreacted");
        assert_eq!(events[0].1["reaction"], "👍");
        assert_eq!(events[1].0, "reacted");
        assert_eq!(events[1].1["userId"], "5");
        assert_eq!(events[1].1["emoji"]["name"], "blob@example.com");
        assert_eq!(events[1].1["emoji"]["url"], "https://example.com/blob.png");

        let unicode = note_updated_events(&body(Some("👍"), None));
        assert_eq!(unicode.len(), 1);
        assert!(unicode[0].1["emoji"].is_null());

        let removed = note_updated_events(&body(None, Some("👍")));
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].0, "unreacted");

        let mut sibling = body(Some("👍"), None);
        sibling.reactor_actor_id = None;
        assert!(note_updated_events(&sibling).is_empty());
    }
}

//! リアルタイム更新のためのストリーミングハブ（#37）。
//!
//! ローカルで発生したイベント（新規ポスト・リアクション・フォロー等）を、
//! 受け取るべきローカルアクターの WebSocket 接続へブロードキャストする。
//! フィルタは各接続側で `recipients` を見て行う。
//!
//! mono バイナリでは api ロールと federation ロールが同一プロセスで動くため、
//! この共有ハブ 1 つを両者の状態に注入して跨いで配信する。

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::broadcast;

use crate::repository::{FollowRepository, ReactionRepository};

/// ストリーミングイベント。`recipients` に含まれるローカルアクターのみが受信する。
#[derive(Clone)]
pub struct StreamEvent {
    pub recipients: Arc<HashSet<i64>>,
    /// クライアントへ送る JSON テキスト（例: `{"type":"note","body":{...}}`）。
    pub payload: Arc<String>,
}

/// プロセス内共有のブロードキャストハブ。
pub struct StreamHub {
    tx: broadcast::Sender<StreamEvent>,
}

impl Default for StreamHub {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(512);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StreamEvent> {
        self.tx.subscribe()
    }

    /// イベントを送出する（購読者がいなくてもエラーにしない）。
    pub fn publish(&self, ev: StreamEvent) {
        let _ = self.tx.send(ev);
    }

    /// 任意種別のイベントを送出する。`{"type":<kind>,"body":<body>}` として配信する。
    pub fn publish_event(&self, recipients: HashSet<i64>, kind: &str, body: serde_json::Value) {
        if recipients.is_empty() {
            return;
        }
        let payload = serde_json::json!({ "type": kind, "body": body }).to_string();
        self.publish(StreamEvent {
            recipients: Arc::new(recipients),
            payload: Arc::new(payload),
        });
    }

    /// 新規ポストイベント（`type: "note"`）を送出する。
    pub fn publish_note(&self, recipients: HashSet<i64>, note_json: &serde_json::Value) {
        self.publish_event(recipients, "note", note_json.clone());
    }
}

/// リアクション追加/切替/取消（ローカル・AP 受信のいずれも）を `noteUpdated` イベントとして
/// 送出する。配信先は投稿の著者 + 著者をフォロー中（承認済み・ローカル）のアクター
/// （`broadcast_new_note` と同じ考え方。「今この投稿を見ている全員」を追跡する仕組みは
/// まだ無いため、既存のリアルタイム配信の範囲に合わせている）。
///
/// `reactor_emoji` は今回のイベント後の「reactor 自身がこの投稿に付けているリアクション」。
/// 切替/追加なら `Some(新しい絵文字)`、取消（他に付け直さなかった場合）なら `None`。
/// 受信側はこれと自分の actor_id を比較して `reactedByMe` を再計算できる（他人のリアクションは
/// 件数のみ更新すればよい）。
pub async fn broadcast_reaction_update(
    stream_hub: &StreamHub,
    follows: &dyn FollowRepository,
    reactions: &dyn ReactionRepository,
    post_id: i64,
    post_author_id: i64,
    reactor_actor_id: i64,
    reactor_emoji: Option<&str>,
) {
    let agg = reactions.aggregate_for_post(post_id).await.unwrap_or_default();
    let reactions_json: Vec<serde_json::Value> = agg
        .into_iter()
        .filter(|(emoji, _, _)| !emoji.is_empty())
        .map(|(emoji, count, emoji_url)| {
            serde_json::json!({ "emoji": emoji, "count": count, "emojiUrl": emoji_url })
        })
        .collect();

    let mut recipients: HashSet<i64> = HashSet::new();
    recipients.insert(post_author_id);
    if let Ok(rows) = follows.find_accepted_local_follower_ids(post_author_id).await {
        recipients.extend(rows);
    }

    stream_hub.publish_event(
        recipients,
        "noteUpdated",
        serde_json::json!({
            "postId": post_id.to_string(),
            "reactions": reactions_json,
            "reactorActorId": reactor_actor_id,
            "reactorEmoji": reactor_emoji,
        }),
    );
}

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

/// Misskey互換のタイムラインチャンネル種別。クライアントが`connect`で指定するチャンネル名
/// （`"homeTimeline"`等）をパースした結果。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChannelKind {
    HomeTimeline,
    LocalTimeline,
    /// Misskey本家の`hybridTimeline`（ソーシャルタイムライン、ホーム+ローカル）。
    HybridTimeline,
    GlobalTimeline,
    UserList(i64),
    Hashtag(String),
}

impl ChannelKind {
    /// `connect`メッセージの`channel`文字列と`params`から`ChannelKind`を組み立てる。
    /// `UserList`/`Hashtag`はparamsが必須で、欠けていれば`None`を返す。
    pub fn parse(channel: &str, params: &serde_json::Value) -> Option<Self> {
        match channel {
            "homeTimeline" => Some(Self::HomeTimeline),
            "localTimeline" => Some(Self::LocalTimeline),
            "hybridTimeline" => Some(Self::HybridTimeline),
            "globalTimeline" => Some(Self::GlobalTimeline),
            "userList" => {
                let list_id = params["listId"].as_str()?.parse::<i64>().ok()?;
                Some(Self::UserList(list_id))
            }
            "hashtag" => {
                let tag = params["tag"].as_str()?;
                if tag.is_empty() {
                    return None;
                }
                Some(Self::Hashtag(tag.to_lowercase()))
            }
            _ => None,
        }
    }
}

/// 新規ノートがどのタイムラインチャンネルに該当するかを表すメタデータ。publish時に1回
/// 構築し、各WSコネクションが自分の購読チャンネル一覧に対して`matches`でO(1)照合する
/// （コネクションごとのDB再問い合わせを発生させないため）。
pub struct ChannelScope {
    pub is_local: bool,
    /// `"public"` / `"unlisted"` / `"followers_only"` のいずれか（`"direct"`はここに来ない。
    /// DM は既存の`recipients`方式のまま`publish_note`で配信される）。
    pub visibility: String,
    /// ホームタイムライン該当者（著者本人 + 承認済みローカルフォロワー）。
    pub home_recipients: Arc<HashSet<i64>>,
    /// 著者をメンバーに含むリストのID集合。
    pub list_ids: Arc<HashSet<i64>>,
    /// 本文由来のハッシュタグ名（正規化済み）。
    pub hashtags: Arc<HashSet<String>>,
}

impl ChannelScope {
    /// 指定チャンネル・閲覧者にこのノートを配信すべきか。各RESTタイムラインクエリ
    /// （`repository::post`の`home_timeline`/`local_timeline`/`social_timeline`/
    /// `global_timeline`、`repository::list`の`timeline`、`repository::hashtag`の
    /// `timeline`）のスコープに合わせている。
    pub fn matches(&self, kind: &ChannelKind, viewer_actor_id: i64) -> bool {
        let is_public_ish = self.visibility != "unlisted" && self.visibility != "followers_only";
        match kind {
            ChannelKind::HomeTimeline => self.home_recipients.contains(&viewer_actor_id),
            ChannelKind::LocalTimeline => self.is_local && is_public_ish,
            ChannelKind::HybridTimeline => {
                (self.is_local && is_public_ish) || self.home_recipients.contains(&viewer_actor_id)
            }
            ChannelKind::GlobalTimeline => is_public_ish,
            ChannelKind::UserList(id) => self.list_ids.contains(id),
            ChannelKind::Hashtag(tag) => {
                self.visibility != "followers_only" && self.hashtags.contains(tag)
            }
        }
    }
}

/// 公開系タイムラインチャンネル向けの配信データ。`ChannelScope`と、各コネクションへ
/// そのまま転送するノートJSONを保持する。
#[derive(Clone)]
pub struct ChannelBroadcast {
    pub scope: Arc<ChannelScope>,
    pub note_json: Arc<serde_json::Value>,
}

/// ストリーミングイベント。`recipients` に含まれるローカルアクターのみが受信する
/// （通知・DM・`noteUpdated`用、既存方式）。`channel` は公開系タイムラインチャンネル向け
/// （新方式、`recipients`とは独立に各コネクションが購読チャンネルで自己判定する）。
#[derive(Clone)]
pub struct StreamEvent {
    pub recipients: Arc<HashSet<i64>>,
    /// クライアントへ送る JSON テキスト（例: `{"type":"note","body":{...}}`）。
    pub payload: Arc<String>,
    pub channel: Option<ChannelBroadcast>,
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
            channel: None,
        });
    }

    /// 新規ポストイベント（`type: "note"`）を送出する（`recipients`方式、DM専用）。
    pub fn publish_note(&self, recipients: HashSet<i64>, note_json: &serde_json::Value) {
        self.publish_event(recipients, "note", note_json.clone());
    }

    /// 公開系タイムラインチャンネル向けにノートを送出する。`recipients`は使わず、各WS
    /// コネクションが自分の購読チャンネル一覧に対し`scope.matches`で個別に判定する。
    pub fn publish_channel_note(&self, scope: ChannelScope, note_json: serde_json::Value) {
        self.publish(StreamEvent {
            recipients: Arc::new(HashSet::new()),
            payload: Arc::new(String::new()),
            channel: Some(ChannelBroadcast {
                scope: Arc::new(scope),
                note_json: Arc::new(note_json),
            }),
        });
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
    let agg = reactions
        .aggregate_for_post(post_id)
        .await
        .unwrap_or_default();
    let reactions_json: Vec<serde_json::Value> = agg
        .into_iter()
        .filter(|(emoji, _, _)| !emoji.is_empty())
        .map(|(emoji, count, emoji_url)| {
            serde_json::json!({ "emoji": emoji, "count": count, "emojiUrl": emoji_url })
        })
        .collect();

    let mut recipients: HashSet<i64> = HashSet::new();
    recipients.insert(post_author_id);
    if let Ok(rows) = follows
        .find_accepted_local_follower_ids(post_author_id)
        .await
    {
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

/// アンケート投票結果の更新（ローカル投票・AP 受信のいずれも）を `pollUpdated` イベントとして
/// 送出する。配信先・考え方は `broadcast_reaction_update` と同じ（投稿の著者 + 著者をフォロー中
/// のローカルアクター）。`poll` は更新後の`posts.poll`そのもの（`votedByMe`は含まない。閲覧者
/// ごとに異なるため、受信側は自分の投票済み選択肢はローカル状態を保ったまま票数のみ更新する）。
pub async fn broadcast_poll_update(
    stream_hub: &StreamHub,
    follows: &dyn FollowRepository,
    post_id: i64,
    post_author_id: i64,
    poll: &serde_json::Value,
) {
    let mut recipients: HashSet<i64> = HashSet::new();
    recipients.insert(post_author_id);
    if let Ok(rows) = follows
        .find_accepted_local_follower_ids(post_author_id)
        .await
    {
        recipients.extend(rows);
    }

    stream_hub.publish_event(
        recipients,
        "pollUpdated",
        serde_json::json!({
            "postId": post_id.to_string(),
            "poll": poll,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::{ChannelKind, ChannelScope};
    use std::collections::HashSet;
    use std::sync::Arc;

    fn scope(
        is_local: bool,
        visibility: &str,
        home: &[i64],
        lists: &[i64],
        tags: &[&str],
    ) -> ChannelScope {
        ChannelScope {
            is_local,
            visibility: visibility.to_string(),
            home_recipients: Arc::new(home.iter().copied().collect()),
            list_ids: Arc::new(lists.iter().copied().collect::<HashSet<i64>>()),
            hashtags: Arc::new(tags.iter().map(|s| s.to_string()).collect()),
        }
    }

    #[test]
    fn channel_kind_parse_recognizes_all_known_channels() {
        let empty = serde_json::json!({});
        assert_eq!(
            ChannelKind::parse("homeTimeline", &empty),
            Some(ChannelKind::HomeTimeline)
        );
        assert_eq!(
            ChannelKind::parse("localTimeline", &empty),
            Some(ChannelKind::LocalTimeline)
        );
        assert_eq!(
            ChannelKind::parse("hybridTimeline", &empty),
            Some(ChannelKind::HybridTimeline)
        );
        assert_eq!(
            ChannelKind::parse("globalTimeline", &empty),
            Some(ChannelKind::GlobalTimeline)
        );
        assert_eq!(
            ChannelKind::parse("userList", &serde_json::json!({"listId": "42"})),
            Some(ChannelKind::UserList(42))
        );
        assert_eq!(
            ChannelKind::parse("hashtag", &serde_json::json!({"tag": "Foo"})),
            Some(ChannelKind::Hashtag("foo".to_string()))
        );
        assert_eq!(ChannelKind::parse("userList", &empty), None);
        assert_eq!(ChannelKind::parse("unknownChannel", &empty), None);
    }

    #[test]
    fn local_timeline_requires_local_origin_and_public_visibility() {
        // リモート投稿（is_local=false）は、フォロー中でも home_recipients に含まれる
        // 閲覧者に対してすら localTimeline へは配信されない（今回の回帰対象そのもの）。
        let remote_public = scope(false, "public", &[1], &[], &[]);
        assert!(!remote_public.matches(&ChannelKind::LocalTimeline, 1));

        let local_public = scope(true, "public", &[], &[], &[]);
        assert!(local_public.matches(&ChannelKind::LocalTimeline, 999));

        let local_unlisted = scope(true, "unlisted", &[], &[], &[]);
        assert!(!local_unlisted.matches(&ChannelKind::LocalTimeline, 999));

        let local_followers_only = scope(true, "followers_only", &[], &[], &[]);
        assert!(!local_followers_only.matches(&ChannelKind::LocalTimeline, 999));
    }

    #[test]
    fn global_timeline_allows_remote_but_not_unlisted_or_followers_only() {
        let remote_public = scope(false, "public", &[], &[], &[]);
        assert!(remote_public.matches(&ChannelKind::GlobalTimeline, 999));

        let remote_unlisted = scope(false, "unlisted", &[], &[], &[]);
        assert!(!remote_unlisted.matches(&ChannelKind::GlobalTimeline, 999));
    }

    #[test]
    fn home_timeline_matches_only_recipients_in_home_set() {
        let s = scope(false, "followers_only", &[1, 2], &[], &[]);
        assert!(s.matches(&ChannelKind::HomeTimeline, 1));
        assert!(s.matches(&ChannelKind::HomeTimeline, 2));
        assert!(!s.matches(&ChannelKind::HomeTimeline, 3));
    }

    #[test]
    fn hybrid_timeline_matches_local_public_or_home_recipient() {
        // ローカルの公開投稿は誰にでも届く。
        let local_public = scope(true, "public", &[], &[], &[]);
        assert!(local_public.matches(&ChannelKind::HybridTimeline, 999));

        // 自分自身のunlisted投稿は home_recipients に自分が含まれるため届く
        // （social_timeline SQLの `p.visibility != 'unlisted' OR p.actor_id = $1` と同義）。
        let own_unlisted = scope(true, "unlisted", &[42], &[], &[]);
        assert!(own_unlisted.matches(&ChannelKind::HybridTimeline, 42));
        assert!(!own_unlisted.matches(&ChannelKind::HybridTimeline, 43));

        // リモートの投稿はフォローしていれば届く。
        let remote_followed = scope(false, "public", &[7], &[], &[]);
        assert!(remote_followed.matches(&ChannelKind::HybridTimeline, 7));
        assert!(!remote_followed.matches(&ChannelKind::HybridTimeline, 8));
    }

    #[test]
    fn user_list_matches_membership_regardless_of_visibility() {
        let s = scope(false, "followers_only", &[], &[10, 20], &[]);
        assert!(s.matches(&ChannelKind::UserList(10), 999));
        assert!(!s.matches(&ChannelKind::UserList(30), 999));
    }

    #[test]
    fn hashtag_matches_tag_but_excludes_followers_only() {
        let public_tagged = scope(true, "public", &[], &[], &["seiran"]);
        assert!(public_tagged.matches(&ChannelKind::Hashtag("seiran".to_string()), 999));
        assert!(!public_tagged.matches(&ChannelKind::Hashtag("other".to_string()), 999));

        let followers_only_tagged = scope(true, "followers_only", &[], &[], &["seiran"]);
        assert!(!followers_only_tagged.matches(&ChannelKind::Hashtag("seiran".to_string()), 999));
    }
}

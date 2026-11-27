//! リアルタイム更新のためのストリーミングハブ（#37）。
//!
//! ローカルで発生したイベント（新規ポスト・リアクション・フォロー等）を、
//! 受け取るべきローカルアクターの WebSocket 接続へブロードキャストする。
//! フィルタは各接続側で `recipients` を見て行う。リアクションの増減（`noteUpdated`）と
//! アンケートの票数（`pollUpdated`）は、それに加えてそのノートを `subNote` で購読している
//! 接続にも届ける（`NoteSubscribers`）。
//!
//! 1プロセスにハブは1つで、そのプロセスの全ロールが共有する。Redis があればハブ同士を
//! Redis でつなぎ、どのプロセスで起きたイベントもすべての接続へ届ける（`event_bus`）。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::event_bus::EventBus;

use crate::repository::{DmRepository, FollowRepository, NoteUpdateTarget, ReactionRepository};

/// Misskey互換のタイムラインチャンネル種別。クライアントが`connect`で指定するチャンネル名
/// （`"homeTimeline"`等）をパースした結果。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChannelKind {
    HomeTimeline,
    LocalTimeline,
    /// 管理者のモデレーション表示のローカルタイムライン（`localTimeline`に加えてひかえめ・
    /// プライベートのローカル投稿も流す）。購読時に管理者であることを確かめる
    /// （`handlers::streaming`）。
    LocalTimelineModeration,
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
            "localTimelineModeration" => Some(Self::LocalTimelineModeration),
            "hybridTimeline" => Some(Self::HybridTimeline),
            "globalTimeline" => Some(Self::GlobalTimeline),
            "userList" => {
                let list_id = params["listId"].as_str()?.parse::<i64>().ok()?;
                Some(Self::UserList(list_id))
            }
            "hashtag" => {
                // `tag` は SPA。Misskey 本家は `q`（OR で並べた AND の組、`[["foo"]]`）で、
                // seiran のタグ購読は1タグだけなので、1組1タグのときだけ受け付ける。
                let tag = match params["tag"].as_str() {
                    Some(tag) => tag,
                    None => match params["q"].as_array().map(Vec::as_slice) {
                        Some([group]) => match group.as_array().map(Vec::as_slice) {
                            Some([tag]) => tag.as_str()?,
                            _ => return None,
                        },
                        _ => return None,
                    },
                };
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
#[derive(Serialize, Deserialize)]
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
    /// 著者が quarantine ロールか拒否サーバーのアクター（`actor_is_kept_off_public_feeds`）で、
    /// グローバル・ハッシュタグに出さない。旧版のプロセスが Redis 経由で送るイベントには
    /// 無いので既定値を持たせる。
    #[serde(default)]
    pub kept_off_public_feeds: bool,
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
            ChannelKind::LocalTimelineModeration => self.is_local,
            ChannelKind::HybridTimeline => {
                (self.is_local && is_public_ish) || self.home_recipients.contains(&viewer_actor_id)
            }
            ChannelKind::GlobalTimeline => is_public_ish && !self.kept_off_public_feeds,
            ChannelKind::UserList(id) => self.list_ids.contains(id),
            ChannelKind::Hashtag(tag) => {
                self.visibility != "followers_only"
                    && !self.kept_off_public_feeds
                    && self.hashtags.contains(tag)
            }
        }
    }
}

/// 公開系タイムラインチャンネル向けの配信データ。`ChannelScope`と、各コネクションへ
/// そのまま転送するノートJSONを保持する。
#[derive(Clone, Serialize, Deserialize)]
pub struct ChannelBroadcast {
    pub scope: Arc<ChannelScope>,
    pub note_json: Arc<serde_json::Value>,
}

/// ストリーミングイベント。`recipients` に含まれるローカルアクターのみが受信する
/// （通知・DM・`noteUpdated`用、既存方式）。`channel` は公開系タイムラインチャンネル向け
/// （新方式、`recipients`とは独立に各コネクションが購読チャンネルで自己判定する）。
/// `broadcast_all` はどちらでもなく、ログイン中の全接続へ無条件配信する第三の方式
/// （`recipients`のように宛先を事前に列挙できない・`channel`のように購読を要求するほどでは
/// ない低頻度イベント向け、例: bio内リンクの非同期解決完了通知）。
#[derive(Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    pub recipients: Arc<HashSet<i64>>,
    /// クライアントへ送る JSON テキスト（例: `{"type":"note","body":{...}}`）。
    pub payload: Arc<String>,
    pub channel: Option<ChannelBroadcast>,
    pub broadcast_all: bool,
    /// `recipients` に加えて、このノートを `subNote` で購読している接続にも配る
    /// （`NoteSubscriptions::admits`）。`None` なら `recipients` だけ。
    /// 無停止更新の間は新旧のプロセスが同じ Redis チャンネルを読むので、この欄を持たない
    /// 旧版のイベントも読めるよう既定値を置く。
    #[serde(default)]
    pub note_subscribers: Option<NoteSubscribers>,
}

/// `subNote` で購読している接続にイベントを配ってよい範囲。
///
/// 購読はクライアントが画面に出しているノートの ID を送るだけなので、閲覧者がそのノートを
/// 見られるかはイベント側に載せた材料で決める（接続ごとに DB を引かない）。載せるのは、
/// 見られる人を投稿者・フォロワーに限らない公開・ホーム（`unlisted`）の投稿だけ。
/// フォロワー限定は見られる人（`post_is_visible_to`）が `recipients`（投稿者とフォロワー）と
/// 同じで、DM は当事者だけに配るので、どちらも購読者へは広げない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteSubscribers {
    pub post_id: i64,
    /// ログインしていない接続にも配るか。`post_is_visible_to` と同じく、未ログインの閲覧者に
    /// 見せるのはローカルアクターの投稿だけ。
    pub anonymous: bool,
}

impl NoteSubscribers {
    /// 投稿の可視性から購読者へ配ってよい範囲を決める（配れなければ `None`）。
    pub fn for_post(post_id: i64, visibility: &str, author_is_local: bool) -> Option<Self> {
        matches!(visibility, "public" | "unlisted").then_some(Self {
            post_id,
            anonymous: author_is_local,
        })
    }
}

/// 1つの WebSocket 接続が `subNote` で購読しているノート。本家 Misskey と同じく、同じノートを
/// 複数の場所に出していれば `subNote` と `unsubNote` を対で数える。
#[derive(Debug, Default)]
pub struct NoteSubscriptions {
    counts: HashMap<i64, u32>,
}

/// 1接続で購読できるノートの数。表示中のノートを数えるだけなので通常は届かない。不正な
/// クライアントが購読を積み上げてサーバーのメモリを使い続けないための上限。
pub const MAX_NOTE_SUBSCRIPTIONS: usize = 2000;

impl NoteSubscriptions {
    /// `id` はクライアントが送ったノート ID の文字列。数値でなければ無視する。
    pub fn subscribe(&mut self, id: &str) {
        let Ok(post_id) = id.parse::<i64>() else {
            return;
        };
        if let Some(count) = self.counts.get_mut(&post_id) {
            *count += 1;
        } else if self.counts.len() < MAX_NOTE_SUBSCRIPTIONS {
            self.counts.insert(post_id, 1);
        }
    }

    pub fn unsubscribe(&mut self, id: &str) {
        let Ok(post_id) = id.parse::<i64>() else {
            return;
        };
        if let Some(count) = self.counts.get_mut(&post_id) {
            *count -= 1;
            if *count == 0 {
                self.counts.remove(&post_id);
            }
        }
    }

    pub fn contains(&self, post_id: i64) -> bool {
        self.counts.contains_key(&post_id)
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// `ev` を購読者としてこの接続へ配ってよいか（`recipients` による配信とは別に判定する）。
    /// `viewer` はこの接続の閲覧者（未ログインなら `None`）。
    pub fn admits(&self, ev: &StreamEvent, viewer: Option<i64>) -> bool {
        ev.note_subscribers
            .is_some_and(|s| (viewer.is_some() || s.anonymous) && self.contains(s.post_id))
    }
}

/// Redis でプロセス間の配信をつなぐときのチャンネル名。
const STREAM_EVENTS_CHANNEL: &str = "seiran:stream:events";
/// 手元の購読者が読み遅れたときに保持しておくイベント数。
const STREAM_EVENTS_CAPACITY: usize = 512;

/// WebSocket 接続へイベントを配るハブ。
pub struct StreamHub {
    bus: EventBus<StreamEvent>,
}

impl Default for StreamHub {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamHub {
    /// このプロセスの中だけで配るハブ。
    pub fn new() -> Self {
        Self {
            bus: EventBus::local(STREAM_EVENTS_CAPACITY),
        }
    }

    /// Redis を通して全プロセスの接続へ配るハブ。
    pub async fn with_redis(client: &redis::Client) -> Result<Self, redis::RedisError> {
        Ok(Self {
            bus: EventBus::with_redis(STREAM_EVENTS_CAPACITY, client, STREAM_EVENTS_CHANNEL)
                .await?,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StreamEvent> {
        self.bus.subscribe()
    }

    /// イベントを送出する（購読者がいなくてもエラーにしない）。
    pub fn publish(&self, ev: StreamEvent) {
        self.bus.publish(ev);
    }

    /// 任意種別のイベントを送出する。`{"type":<kind>,"body":<body>}` として配信する。
    pub fn publish_event(&self, recipients: HashSet<i64>, kind: &str, body: serde_json::Value) {
        self.publish_note_event(recipients, None, kind, body);
    }

    /// `publish_event` に加えて、`subscribers` の範囲でノートの購読者にも配る。
    pub fn publish_note_event(
        &self,
        recipients: HashSet<i64>,
        subscribers: Option<NoteSubscribers>,
        kind: &str,
        body: serde_json::Value,
    ) {
        if recipients.is_empty() && subscribers.is_none() {
            return;
        }
        let payload = serde_json::json!({ "type": kind, "body": body }).to_string();
        self.publish(StreamEvent {
            recipients: Arc::new(recipients),
            payload: Arc::new(payload),
            channel: None,
            broadcast_all: false,
            note_subscribers: subscribers,
        });
    }

    /// ログイン中の全接続へ無条件配信する（購読不要）。「誰が見ているか」を追跡するコストに
    /// 見合わない低頻度イベント向け（bio/profile_fields中のURL解決完了通知等）。
    /// `{"type":<kind>,"body":<body>}` として配信する。
    pub fn publish_broadcast(&self, kind: &str, body: serde_json::Value) {
        let payload = serde_json::json!({ "type": kind, "body": body }).to_string();
        self.publish(StreamEvent {
            recipients: Arc::new(HashSet::new()),
            payload: Arc::new(payload),
            channel: None,
            broadcast_all: true,
            note_subscribers: None,
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
            broadcast_all: false,
            note_subscribers: None,
        });
    }
}

fn reaction_counts(agg: Vec<(String, i64, Option<String>)>) -> Vec<ReactionCount> {
    agg.into_iter()
        .filter(|(emoji, _, _)| !emoji.is_empty())
        .map(|(emoji, count, emoji_url)| ReactionCount {
            emoji,
            count,
            emoji_url,
        })
        .collect()
}

/// 1人のリアクションの変化（`broadcast_reaction_update`/`broadcast_dm_reaction_update`の入力）。
#[derive(Debug, Clone, Copy)]
pub struct ReactorChange<'a> {
    pub actor_id: i64,
    /// 変化の後にこの人が付けているリアクション（取り消しなら`None`）。
    pub emoji: Option<&'a str>,
    /// 変化の前に付けていたリアクション（新規なら`None`）。1投稿1リアクションなので、切り替え・
    /// 取り消しで消えた絵文字がこれ。Mastodon 互換ストリーミングは絵文字ごとの件数を送るので、
    /// 0件になった絵文字を知らせるのに使う（集計の`reactions`には0件の絵文字が載らない）。
    pub previous_emoji: Option<&'a str>,
}

/// `noteUpdated`の本文。SPA と Misskey 互換のストリーミング、Mastodon 互換のストリーミング
/// （`emoji_reaction`への組み直し）が読む。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteUpdatedBody {
    pub post_id: String,
    pub reactions: Vec<ReactionCount>,
    pub reactor_actor_id: Option<String>,
    pub reactor_emoji: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reactor_previous_emoji: Option<String>,
}

/// `noteUpdated`の絵文字ごとの件数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionCount {
    pub emoji: String,
    pub count: i64,
    pub emoji_url: Option<String>,
}

/// `noteUpdated`のイベント名。
pub const NOTE_UPDATED_EVENT: &str = "noteUpdated";

/// `broadcast_reaction_update`/`broadcast_dm_reaction_update`共通の送出部分。
fn publish_reaction_update(
    stream_hub: &StreamHub,
    recipients: HashSet<i64>,
    subscribers: Option<NoteSubscribers>,
    post_id: i64,
    reactions: Vec<ReactionCount>,
    reactor: Option<ReactorChange<'_>>,
) {
    let body = NoteUpdatedBody {
        post_id: post_id.to_string(),
        reactions,
        reactor_actor_id: reactor.map(|r| r.actor_id.to_string()),
        reactor_emoji: reactor.and_then(|r| r.emoji).map(str::to_owned),
        reactor_previous_emoji: reactor.and_then(|r| r.previous_emoji).map(str::to_owned),
    };
    let Ok(body) = serde_json::to_value(&body) else {
        return;
    };
    stream_hub.publish_note_event(recipients, subscribers, NOTE_UPDATED_EVENT, body);
}

/// リアクション追加/切替/取消（ローカル・AP 受信のいずれも）を `noteUpdated` イベントとして
/// 送出する。配信先は、そのノートを `subNote` で購読している（画面に出している）接続
/// （`NoteSubscribers`、公開・ホームの投稿のみ）と、投稿の著者 + 著者をフォロー中（承認済み・
/// ローカル）のアクター（購読を送らない旧版のクライアントにも届くよう残している）。
/// **`direct`可視性の投稿には使わないこと**（フォロワーはDMの当事者ではなく、DMの存在・リアクション内容が無関係な
/// 第三者のWS接続に漏れる。`direct`投稿には`broadcast_dm_reaction_update`を使う）。
///
/// `reactor.emoji` は今回のイベント後の「reactor 自身がこの投稿に付けているリアクション」。
/// 切替/追加なら `Some(新しい絵文字)`、取消（他に付け直さなかった場合）なら `None`。
/// 受信側はこれと自分の actor_id を比較して `reactedByMe` を再計算できる（他人のリアクションは
/// 件数のみ更新すればよい）。
///
/// 件数はブリッジ関係の投稿と合算した値なので、それらの投稿（`note_update_targets`）の表示にも、
/// その投稿者とフォロワーへ同じ集計を送る。こちらは `reactorActorId` を null にする
/// （reactor が付けたのは別の投稿なので、受信側でその投稿の `reactedByMe` を変えさせない）。
pub async fn broadcast_reaction_update(
    stream_hub: &StreamHub,
    follows: &dyn FollowRepository,
    reactions: &dyn ReactionRepository,
    post_id: i64,
    post_author_id: i64,
    reactor: ReactorChange<'_>,
) {
    let agg = reactions
        .aggregate_for_post(post_id)
        .await
        .unwrap_or_default();
    let counts = reaction_counts(agg);
    let targets = reactions
        .note_update_targets(post_id)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("[streaming] noteUpdated の配信先の取得に失敗: {}", e);
            Vec::new()
        });

    // 対象の投稿が引けなくても（DB エラー）、著者とフォロワーには従来どおり届ける。
    let own = targets.iter().find(|t| t.post_id == post_id);
    publish_reaction_update(
        stream_hub,
        author_and_followers(follows, post_author_id).await,
        own.and_then(subscribers_of),
        post_id,
        counts.clone(),
        Some(reactor),
    );

    for sibling in targets.iter().filter(|t| t.post_id != post_id) {
        publish_reaction_update(
            stream_hub,
            author_and_followers(follows, sibling.author_id).await,
            subscribers_of(sibling),
            sibling.post_id,
            counts.clone(),
            None,
        );
    }
}

fn subscribers_of(target: &NoteUpdateTarget) -> Option<NoteSubscribers> {
    NoteSubscribers::for_post(target.post_id, &target.visibility, target.author_is_local)
}

async fn author_and_followers(follows: &dyn FollowRepository, author_id: i64) -> HashSet<i64> {
    let mut recipients: HashSet<i64> = HashSet::new();
    recipients.insert(author_id);
    if let Ok(rows) = follows.find_accepted_local_follower_ids(author_id).await {
        recipients.extend(rows);
    }
    recipients
}

/// `broadcast_reaction_update`のDM（`visibility='direct'`）専用版。配信先を
/// DMの参加者（投稿者 + `post_recipients`の宛先）のみに絞る。通常投稿版のようにフォロワーを
/// 含めると、DMメッセージへのリアクション内容が無関係な第三者に漏れてしまう
/// （`ap::deliver::reaction::resolve_reaction_targets`のAP配送側で先に見つかった問題と同種、
/// `docs/protocols.md` 9節参照）。
pub async fn broadcast_dm_reaction_update(
    stream_hub: &StreamHub,
    dm: &dyn DmRepository,
    reactions: &dyn ReactionRepository,
    post_id: i64,
    post_author_id: i64,
    reactor: ReactorChange<'_>,
) {
    let agg = reactions
        .aggregate_for_post(post_id)
        .await
        .unwrap_or_default();
    let counts = reaction_counts(agg);

    let mut recipients: HashSet<i64> = HashSet::new();
    recipients.insert(post_author_id);
    if let Ok(rows) = dm.recipient_ids(post_id).await {
        recipients.extend(rows);
    }

    publish_reaction_update(stream_hub, recipients, None, post_id, counts, Some(reactor));
}

/// アンケート投票結果の更新（ローカル投票・AP 受信のいずれも）を `pollUpdated` イベントとして
/// 送出する。配信先・考え方は `broadcast_reaction_update` と同じ（ノートの購読者（公開・ホームの
/// 投稿のみ）と、投稿の著者 + 著者をフォロー中のローカルアクター）。`poll` は更新後の`posts.poll`そのもの（`votedByMe`は含まない。閲覧者
/// ごとに異なるため、受信側は自分の投票済み選択肢はローカル状態を保ったまま票数のみ更新する）。
pub async fn broadcast_poll_update(
    stream_hub: &StreamHub,
    follows: &dyn FollowRepository,
    db: &sqlx::PgPool,
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
    let subscribers =
        match crate::repository::post::visibility_and_author_locality(db, post_id).await {
            Ok(found) => found.and_then(|(visibility, author_is_local)| {
                NoteSubscribers::for_post(post_id, &visibility, author_is_local)
            }),
            Err(e) => {
                tracing::error!("[streaming] pollUpdated の配信先の取得に失敗: {}", e);
                None
            }
        };

    stream_hub.publish_note_event(
        recipients,
        subscribers,
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
            kept_off_public_feeds: false,
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
        assert_eq!(
            ChannelKind::parse("hashtag", &serde_json::json!({"q": [["Foo"]]})),
            Some(ChannelKind::Hashtag("foo".to_string()))
        );
        assert_eq!(
            ChannelKind::parse("hashtag", &serde_json::json!({"q": [["a", "b"]]})),
            None
        );
        assert_eq!(ChannelKind::parse("userList", &empty), None);
        assert_eq!(ChannelKind::parse("unknownChannel", &empty), None);
    }

    #[test]
    fn local_timeline_requires_local_origin_and_public_visibility() {
        // リモート投稿（is_local=false）は、フォロー中でも home_recipients に含まれる
        // 閲覧者に対してすら localTimeline へは配信されない。
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
    fn local_timeline_moderation_includes_restricted_local_posts() {
        for visibility in ["public", "unlisted", "followers_only"] {
            let local = scope(true, visibility, &[], &[], &[]);
            assert!(local.matches(&ChannelKind::LocalTimelineModeration, 999));
        }
        let remote = scope(false, "public", &[1], &[], &[]);
        assert!(!remote.matches(&ChannelKind::LocalTimelineModeration, 1));
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

    #[test]
    fn kept_off_public_feeds_excludes_global_and_hashtag_only() {
        let mut s = scope(false, "public", &[7], &[10], &["seiran"]);
        s.kept_off_public_feeds = true;
        assert!(!s.matches(&ChannelKind::GlobalTimeline, 7));
        assert!(!s.matches(&ChannelKind::Hashtag("seiran".to_string()), 7));
        assert!(s.matches(&ChannelKind::HomeTimeline, 7));
        assert!(s.matches(&ChannelKind::UserList(10), 7));
    }
}

#[cfg(test)]
mod note_subscription_tests {
    use super::{NoteSubscribers, NoteSubscriptions, StreamEvent, MAX_NOTE_SUBSCRIPTIONS};
    use std::collections::HashSet;
    use std::sync::Arc;

    fn event(note_subscribers: Option<NoteSubscribers>) -> StreamEvent {
        StreamEvent {
            recipients: Arc::new(HashSet::new()),
            payload: Arc::new(String::new()),
            channel: None,
            broadcast_all: false,
            note_subscribers,
        }
    }

    #[test]
    fn only_public_and_unlisted_posts_reach_subscribers() {
        assert!(NoteSubscribers::for_post(1, "public", false).is_some());
        assert!(NoteSubscribers::for_post(1, "unlisted", false).is_some());
        assert!(NoteSubscribers::for_post(1, "followers_only", true).is_none());
        assert!(NoteSubscribers::for_post(1, "direct", true).is_none());
    }

    #[test]
    fn subscriptions_are_counted_in_pairs() {
        let mut subs = NoteSubscriptions::default();
        subs.subscribe("10");
        subs.subscribe("10");
        subs.unsubscribe("10");
        assert!(subs.contains(10));
        subs.unsubscribe("10");
        assert!(!subs.contains(10));
        subs.subscribe("not-a-number");
        assert!(subs.is_empty());
    }

    #[test]
    fn admits_subscribed_note_and_respects_anonymous_flag() {
        let mut subs = NoteSubscriptions::default();
        subs.subscribe("10");
        let remote = event(NoteSubscribers::for_post(10, "public", false));
        assert!(subs.admits(&remote, Some(1)));
        assert!(!subs.admits(&remote, None));
        let local = event(NoteSubscribers::for_post(10, "public", true));
        assert!(subs.admits(&local, None));
        assert!(!subs.admits(
            &event(NoteSubscribers::for_post(11, "public", true)),
            Some(1)
        ));
        assert!(!subs.admits(&event(None), Some(1)));
    }

    #[test]
    fn subscription_count_is_capped() {
        let mut subs = NoteSubscriptions::default();
        for i in 0..(MAX_NOTE_SUBSCRIPTIONS as i64 + 10) {
            subs.subscribe(&i.to_string());
        }
        assert!(!subs.contains(MAX_NOTE_SUBSCRIPTIONS as i64));
        assert!(subs.contains(0));
    }

    #[test]
    fn events_without_note_subscribers_field_still_deserialize() {
        let json = r#"{"recipients":[1],"payload":"x","channel":null,"broadcast_all":false}"#;
        let ev: StreamEvent = serde_json::from_str(json).unwrap();
        assert!(ev.note_subscribers.is_none());
    }
}

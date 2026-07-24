use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use std::time::Duration;

// ==========================================
// 1. データベース・共通構造体定義
// ==========================================

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct DbPost {
    pub id: i64,
    pub actor_id: i64,
    pub body: String,
    pub reply_to_post_id: Option<i64>,
    pub repost_of_post_id: Option<i64>,
    pub quote_of_post_id: Option<i64>,
    pub seiran_post_uuid: Option<String>,
    pub parent_original_post_id: Option<i64>,
    pub ap_object_id: Option<String>,
    pub at_uri: Option<String>,
    pub at_cid: Option<String>,
    pub metadata: serde_json::Value,
    pub deleted_at: Option<DateTime<Utc>>,
    pub atp_tombstone_cid: Option<String>,
    pub created_at: DateTime<Utc>,
    pub inserted_at: DateTime<Utc>,
}

// ==========================================
// 3. 検索ステート ＆ セッションマネージャー
// ==========================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchSession {
    pub query: String,
    pub appview_cursor: Option<String>,
    pub unreturned_appview_posts: Vec<DbPost>,
    pub last_accessed_at: DateTime<Utc>,
    pub appview_exhausted: bool,
}

#[derive(Debug)]
pub enum StoreError {
    NotFound,
    ConnectionError(String),
    SerializationError(String),
    Internal(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NotFound => write!(f, "Session not found"),
            StoreError::ConnectionError(msg) => write!(f, "Store connection error: {}", msg),
            StoreError::SerializationError(msg) => write!(f, "Store serialization error: {}", msg),
            StoreError::Internal(msg) => write!(f, "Store internal error: {}", msg),
        }
    }
}

impl std::error::Error for StoreError {}

#[async_trait]
pub trait SessionStore: Send + Sync {
    /// セッションID（UUID）に紐付く検索セッションを取得します。
    async fn get_session(&self, session_id: &Uuid) -> Result<Option<SearchSession>, StoreError>;
    
    /// 検索セッションを保存または更新（TTL付き）します。
    async fn set_session(&self, session_id: Uuid, session: SearchSession, ttl: Duration) -> Result<(), StoreError>;
    
    /// 指定された検索セッションを破棄します。
    async fn delete_session(&self, session_id: &Uuid) -> Result<(), StoreError>;
}

// ==========================================
// 4. 非同期ジョブキュー (Job Queue)
// ==========================================

/// AP 配送ジョブ（`Job::ApDelivery`）の配送内容。
///
/// 「どのアクティビティを配送するか」（what）だけを持ち、宛先解決・署名 POST（how）は
/// ジョブハンドラ側（`jobs::ap_delivery` → `ap::deliver`）が行う。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApDeliveryKind {
    /// Create(Note) をフォロワーへ配送する。
    /// `body` はメンション変換済み等の上書き本文（`None` なら DB の posts.body を使用）。
    PostToFollowers {
        post_id: i64,
        body: Option<String>,
        quote_url: Option<String>,
        in_reply_to: Option<String>,
    },
    /// DM（`visibility='direct'`）投稿を、宛先（`post_recipients`）の中のFediアクターへ
    /// のみ配送する（フォロワーコレクションではなく宛先個人のinboxのみ）。
    DirectMessage { post_id: i64 },
    /// Announce（リポスト）を配送する。
    Announce { post_id: i64, original_ap_object_id: String },
    /// Undo(Announce)（リポスト取り消し）を配送する。
    UndoAnnounce { announce_post_id: i64, original_ap_object_id: String },
    /// Delete(Note)（Bsky ネイティブポストのリポスト取り消し）を配送する。
    /// Bsky リモートポストは Fedi 側に Announce ではなく `PostToFollowers` の
    /// Create(Note) フォールバックとして配信されるため、取り消し時も Announce の
    /// Undo ではなく、その Note（`https://{domain}/notes/{post_id}`）自体の
    /// Delete を送る必要がある。
    DeleteNote { post_id: i64 },
    /// Like/EmojiReact を配送する。`undo_prev` があれば先に旧リアクションの Undo を配送する（切替）。
    /// `emoji_url` はカスタム絵文字（`:shortcode:`）の画像 URL。Unicode 絵文字は `None`。
    /// Misskey/Fedibird 互換の `tag: [{type: Emoji, ...}]` 組み立てに使う（`ap::deliver::build_reaction_object`）。
    Reaction {
        post_id: i64,
        activity_id: String,
        content: String,
        emoji_url: Option<String>,
        undo_prev: Option<PrevApReaction>,
    },
    /// Undo(Like/EmojiReact)（リアクション取り消し）を配送する。
    UndoReaction { post_id: i64, prev_activity_id: String, content: String, emoji_url: Option<String> },
    /// Update(Person)（プロフィール更新）を配送する。
    UpdateActor,
    /// Delete(Actor)（退会）を配送する。
    DeleteActor,
}

/// リアクション切替時に取り消す旧リアクションの情報。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrevApReaction {
    pub activity_id: String,
    pub content: String,
    pub emoji_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Job {
    /// 新規フォローされたアクターの過去ログ（最大300件）を取得・保存する
    ActorHistorySync { ap_uri: Option<String>, at_did: Option<String> },

    /// ローカルアクターの AP アクティビティ（投稿・リポスト・リアクション・プロフィール更新等）を
    /// Fedi フォロワーの Inbox へ配送する
    ApDelivery { actor_id: i64, kind: ApDeliveryKind },

    /// 外部（APのInbox等）から届いたアクティビティを非同期解析・DB保存する
    InboundActivityProcess { raw_activity: String },
    
    /// リモートseiranアクターのハンドシェイク検証、Webfinger解決、アバター等プロキシ
    ActorMetadataResolve { actor_id: i64 },
    
    /// AT Protocolリポジトリのコミットと、リレーへの通知
    AtpRepositoryPublish { actor_id: i64, commit_type: String },

    /// Bsky公式動画パイプライン（app.bsky.video.uploadVideo）の完了待ち。
    /// getJobStatusを1回叩き、未完了ならErrを返してリトライさせる。
    BskyVideoPoll { media_file_id: i64 },

    /// リスト機能（#63）: list-relay 仮想アクターによる代理フォロー/アンフォローの同期。
    /// `want_follow: true` はリストへの初回参照時、`false` は参照が0件になった時に積む
    /// （参照カウントの判定は呼び出し側の `ListRepository::actor_referenced_by_any_list` で行う）。
    ProxyFollowSync { target_actor_id: i64, want_follow: bool },

    /// 退会処理: 自分がフォローしていた相手（フォロイー）全員への一括アンフォロー
    /// （ATPフォロー解除コミット + AP Undo Follow配送 + follows削除）。フォロー数に
    /// 比例して時間がかかるため、Delete(Actor)配送（`ApDelivery`）と同様にジョブ化する。
    AccountWithdrawUnfollowAll { actor_id: i64, username: String },

    /// 動画添付を含む投稿の Bsky ATP コミットを、動画パイプライン結合
    /// （`media_files.bsky_video_status`）が確定状態（`ready`/`failed`）になるまで
    /// 遅延する。投稿作成時点でまだトランスコード中の動画に対して即座に `commit_post`
    /// すると、その時点の状態でしか判定できず常に `external` フォールバックになって
    /// しまうため（2026-07-17 マイケル指摘・実機再現確認）。`reply_root`/`reply_parent`
    /// は `(uri, cid)` のタプルで、リプライでない場合は両方 `None`。
    BskyPostCommitDeferred {
        actor_id: i64,
        post_id: i64,
        text: String,
        attachment_ids: Vec<i64>,
        reply_root: Option<(String, String)>,
        reply_parent: Option<(String, String)>,
        /// 投稿作成時点の時刻。ATP レコードの `createdAt` は実行時ではなく
        /// この時刻を使う（ジョブの実行がずれても投稿日時がずれないように）。
        now: DateTime<Utc>,
    },

    /// Bsky投稿のメンションfacetに含まれるDIDが未解決（ローカル`actors`に無い）の場合、
    /// AppView からプロフィールを取得して upsert する。`posts.mention_facets` に保存された
    /// DID は表示時（`NoteResponse` 生成時）に都度解決を試みるため、このジョブは
    /// 「次回表示までに解決を終えておく」ためのベストエフォート先行解決。
    ResolveBskyMention { did: String },

    /// DM（`visibility='direct'`）投稿を、宛先の中のBskyアクターへ`chat.bsky.convo.sendMessage`
    /// で送信する。convoIdが`bsky_convo_links`に未キャッシュなら`getConvoForMembers`で先に解決する。
    BskyDmSend { post_id: i64 },

    /// リモート Fedi アクターの followers/following OrderedCollection を全件取得し、
    /// `remote_follow_snapshots` へキャッシュする（#68）。プロフィール表示時の短タイムアウト
    /// 同期取得が失敗/タイムアウトした場合のフォールバックとして積まれる。
    /// `direction` は `"following"` または `"followers"`。
    RemoteFollowListSync { actor_id: i64, direction: String },
}

/// `JobQueue::dequeue_blocking` が返す、実行対象ジョブとそのメタデータ。
/// `priority`/`attempt` はリトライ時に同じ値で `enqueue_retry` へ引き継ぐために保持する。
#[derive(Debug, Clone)]
pub struct QueuedJob {
    pub job: Job,
    pub priority: i32,
    /// これまでの試行回数（0 始まり）。リトライ設定の上限判定・バックオフ計算に使う。
    pub attempt: u32,
}

#[async_trait]
pub trait JobQueue: Send + Sync {
    /// ジョブを非同期キューに追加します（初回投入。attempt=0 相当）。
    /// 優先度は値が大きいほど先に処理される。
    async fn enqueue(&self, job: Job, priority: i32) -> Result<(), String>;

    /// リトライ用の再投入。`delay` 経過後に実行可能になる。
    /// `attempt` は次に行う試行の番号（1 始まり）で、Worker がリトライ上限判定に使う。
    ///
    /// InMemory 実装はプロセス内 sleep で遅延を実現するため、プロセス再起動で
    /// リトライ待ち状態は失われる（開発用途では許容）。Redis 実装は遅延キュー
    /// （sorted set）に載せるため、Worker プロセスが再起動してもリトライ状態は残る。
    async fn enqueue_retry(&self, job: Job, priority: i32, attempt: u32, delay: Duration) -> Result<(), String>;

    /// 実行可能なジョブが出るまでブロックして 1 件取得する。
    /// WorkerEngine のメインループが呼ぶ。バックエンドを問わず同じインターフェースで
    /// 動くことで、WorkerEngine は InMemory / Redis のどちらでも同一コードで動作する。
    async fn dequeue_blocking(&self) -> QueuedJob;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Job は将来の Redis バックエンド（フェーズ8）でプロセス間をシリアライズ経由で
    /// 移動するため、serde 往復が壊れていないことを保証する。
    #[test]
    fn job_serde_round_trip() {
        let jobs = vec![
            Job::ActorHistorySync { ap_uri: Some("https://a.example/users/x".into()), at_did: None },
            Job::ApDelivery {
                actor_id: 1,
                kind: ApDeliveryKind::PostToFollowers {
                    post_id: 2,
                    body: Some("hello".into()),
                    quote_url: None,
                    in_reply_to: Some("https://b.example/notes/3".into()),
                },
            },
            Job::ApDelivery {
                actor_id: 1,
                kind: ApDeliveryKind::Reaction {
                    post_id: 2,
                    activity_id: "https://a.example/activities/r1".into(),
                    content: "🎉".into(),
                    emoji_url: None,
                    undo_prev: Some(PrevApReaction {
                        activity_id: "https://a.example/activities/r0".into(),
                        content: "❤️".into(),
                        emoji_url: None,
                    }),
                },
            },
            Job::ApDelivery { actor_id: 1, kind: ApDeliveryKind::DeleteActor },
            Job::InboundActivityProcess { raw_activity: "{}".into() },
            Job::AtpRepositoryPublish { actor_id: 1, commit_type: "create_post".into() },
            Job::BskyVideoPoll { media_file_id: 9 },
            Job::BskyPostCommitDeferred {
                actor_id: 1,
                post_id: 2,
                text: "hello".into(),
                attachment_ids: vec![3, 4],
                reply_root: Some(("at://did:plc:x/app.bsky.feed.post/a".into(), "bafyrei...".into())),
                reply_parent: Some(("at://did:plc:x/app.bsky.feed.post/b".into(), "bafyrei...".into())),
                now: Utc::now(),
            },
        ];
        for job in jobs {
            let json = serde_json::to_string(&job).expect("serialize");
            let back: Job = serde_json::from_str(&json).expect("deserialize");
            // Job は PartialEq 未実装のため、再シリアライズ結果の一致で確認する
            assert_eq!(json, serde_json::to_string(&back).unwrap());
        }
    }
}

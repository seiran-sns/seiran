use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

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
    async fn set_session(
        &self,
        session_id: Uuid,
        session: SearchSession,
        ttl: Duration,
    ) -> Result<(), StoreError>;

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
    Announce {
        post_id: i64,
        original_ap_object_id: String,
    },
    /// Undo(Announce)（リポスト取り消し）を配送する。
    UndoAnnounce {
        announce_post_id: i64,
        original_ap_object_id: String,
    },
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
    /// リモートFediアンケートへの回答。選択肢ごとに Create(Note) を投票先へ配送する。
    PollVote {
        post_id: i64,
        option_names: Vec<String>,
    },
    /// Undo(Like/EmojiReact)（リアクション取り消し）を配送する。
    UndoReaction {
        post_id: i64,
        prev_activity_id: String,
        content: String,
        emoji_url: Option<String>,
    },
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
    ActorHistorySync {
        ap_uri: Option<String>,
        at_did: Option<String>,
    },

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
    ProxyFollowSync {
        target_actor_id: i64,
        want_follow: bool,
    },

    /// 退会処理: 自分がフォローしていた相手（フォロイー）全員への一括アンフォロー
    /// （ATPフォロー解除コミット + AP Undo Follow配送 + follows削除）。フォロー数に
    /// 比例して時間がかかるため、Delete(Actor)配送（`ApDelivery`）と同様にジョブ化する。
    AccountWithdrawUnfollowAll { actor_id: i64, username: String },

    /// フォロー承認制（鍵アカウント）をOFFに切り替えた際、その時点で存在した承認待ち
    /// （`follows.status = 'pending'`）フォローリクエスト全件を一括承認する
    /// （`follow_approval::approve_pending_follow`）。フォロワー数に比例して時間がかかりうる
    /// （ローカルフォロワーはATPコミットを、Fediフォロワーは AP Accept 送信を伴うため）
    /// ため、`AccountWithdrawUnfollowAll` と同様にジョブ化する。
    FollowRequestsBulkAccept { actor_id: i64 },

    /// Bsky embedとして選択された（#227、明示選択または省略時の固定優先順位）動画/音声添付の
    /// Bsky ATP コミットを、動画パイプライン結合（`media_files.bsky_video_status`）が確定状態
    /// （`ready`/`failed`）になるまで遅延する。投稿作成時点でまだトランスコード中の動画に
    /// 対して即座に `commit_post` すると、その時点の状態でしか判定できず常に `external`
    /// フォールバックになってしまうため（2026-07-17 マイケル指摘・実機再現確認）。
    /// `pending_media_file_id` は選択が解決した先の`media_files.id`1件（`resolve_bsky_embed`の
    /// 優先順位判定結果を `posts.pending_bsky_media_file_id` へ投稿作成時点で永続化した値を
    /// そのまま渡す）。本文・投稿時刻・リプライ先at_uri/at_cidはジョブのペイロードには
    /// 持たせず、ハンドラが `post_id` から `posts` テーブルを都度参照して取得する
    /// （プロセス再起動でジョブのペイロードが失われても、`post_id`さえ分かれば起動時
    /// リカバリで完全に再現できるようにするため。詳細は`docs/architecture.md`参照）。
    BskyPostCommitDeferred {
        actor_id: i64,
        post_id: i64,
        pending_media_file_id: i64,
    },

    /// DM（`visibility='direct'`）投稿を、宛先の中のBskyアクターへ`chat.bsky.convo.sendMessage`
    /// で送信する。convoIdが`bsky_convo_links`に未キャッシュなら`getConvoForMembers`で先に解決する。
    BskyDmSend { post_id: i64 },

    /// リモート Fedi アクターの followers/following OrderedCollection を全件取得し、
    /// `remote_follow_snapshots` へキャッシュする（#68）。プロフィール表示時の短タイムアウト
    /// 同期取得が失敗/タイムアウトした場合のフォールバックとして積まれる。
    /// `direction` は `"following"` または `"followers"`。
    RemoteFollowListSync { actor_id: i64, direction: String },

    /// リモート followers/following 一覧中、ローカル `actors` に未登録の actor URI を
    /// 解決してプロフィールを upsert する（#68 マイケル指摘: 未知アクターもジョブ化）。
    /// フォロー関係は作らず、表示のリッチ化（アバター・表示名等）のみが目的。
    RemoteActorResolve { uri: String },

    /// リモートFediアクターのfeatured collection（ピン留め投稿, #61）を同期する。
    /// DB登録済みアクターのプロフィール表示のたびに積まれ、表示自体は常にDB上の
    /// 既存`pinned_posts`をそのまま返す（「表示時再検証」パターン、`AlsoKnownAsVerify`と同様）。
    /// Authorized Fetch（secure mode）を要求するリモートだと同期フェッチが数秒かかることが
    /// あり、プロフィール表示のたびにブロッキングで待つのは体感速度を損なうため（2026-08-31
    /// マイケル指摘）、ジョブへ切り出した。初回アクセス時（DB未登録アクターの初回upsert
    /// 直後）だけは`handlers::users::fetch_remote_profile`が同期で取得する。
    RemoteFeaturedSync { actor_id: i64 },

    /// プロフィールの「別のアカウント」（alsoKnownAs、AP Moveの語彙をプロフィール表示・
    /// 相互検証用途に転用したseiran独自拡張、`docs/protocols.md`参照）の相互検証。
    /// プロフィール表示のたびに積まれ、キャッシュ済みの検証結果を非同期で更新する。
    AlsoKnownAsVerify {
        owner_actor_id: i64,
        target_actor_id: i64,
    },

    /// プロフィールの「別のアカウント」表示: リモートFediアクター自身のAP actor文書が
    /// 公開している`alsoKnownAs`を`actor_also_known_as`へ同期する（本人の自己申告を
    /// そのまま取り込む）。同期後、取り込んだ各エントリについて`AlsoKnownAsVerify`を積む。
    RemoteAlsoKnownAsSync { owner_actor_id: i64 },

    /// Fediverseリレー参加機能（#140）: relay-agent 仮想アクターによるリレーへの
    /// Follow/Undo送信の同期。`want_follow: true` はリレー登録時、`false` は削除時に積む。
    RelayFollowSync { relay_id: i64, want_follow: bool },

    /// Fedi受信投稿の本文中URLへアクセスし、OGPメタタグ（og:title/description/image）と
    /// oEmbed discoveryによる埋め込みプレーヤー情報（`embed_src`/`embed_type`、ホワイトリスト
    /// 判定込み）を取得して`post_link_cards`へ保存する。取得できなければ静かに諦める
    /// （リトライ後も失敗し続けたらそのURLはカード無しのまま）。
    OgpFetch {
        post_id: i64,
        url: String,
        position: i16,
    },

    /// Bsky受信投稿（`app.bsky.embed.external`）のURLカードに対し、oEmbed discoveryで
    /// 見つかった埋め込みプレーヤーのiframe srcを非同期に追記する。Bskyのexternal embedには
    /// iframe情報が無く、title/description/thumbnailは既に同期的にINSERT済みのため、
    /// このジョブはUPDATEのみ行う（INSERTは行わない）。取得できなければ諦めてembed_src無し
    /// のまま（一般URLカード表示にフォールバック）。
    LinkCardEmbedResolve {
        post_id: i64,
        position: i16,
        url: String,
    },

    /// リモートインスタンス（Fedi、`actors.domain`単位）のnodeinfoを取得し
    /// `remote_instance_meta` へキャッシュする（#NoteCardリモートサーバー表示）。
    /// notes API / Misskey互換API がキャッシュ未登録のドメインを見つけた際に積む。
    RemoteInstanceInfoResolve { domain: String },

    /// フォローインポート（設定画面からの一括フォロー、隠し仕様でMisskeyエクスポート
    /// CSVの1列目のみを識別子として読む）の自己再enqueue型ジョブ。`follow_import_items`
    /// に `pending` が残っていれば1件処理し、成功・失敗を問わず自分自身を再度積む。
    /// レート制限（`check_follow_rate_limit`）に引っかかった場合は該当itemを`pending`の
    /// まま5分後の`enqueue_retry`で自分自身を再投入する（WorkerEngineの指数バックオフ・
    /// attemptカウンタは消費しない）。対象が尽きるか`follow_import_requests.status`が
    /// `running`でなくなったら（完了/キャンセル）再enqueueせず終了する。
    FollowImportProcess { request_id: i64 },

    /// リモート（seiranユーザー所有でない）Bskyリストの全メンバーDIDを`app.bsky.graph.getList`
    /// から取得し、`bsky_remote_list_membership_cache`へ24時間TTLで保存する。
    /// threadgate の listRule 評価（`docs/protocols.md`参照）でキャッシュ未登録/期限切れの
    /// リストを見つけた際に積む。ローカルseiranユーザー所有のリストは`lists`/`list_members`に
    /// 既に答えがあるためこのジョブの対象にならない。
    BskyListMembershipResolve { list_uri: String },

    /// リモートアンケート（AP Question）の生存監視フォールバック。Update(Question)を
    /// 送ってこない実装への保険として、締切前かつ長時間未フェッチのpollを表示読み込み時に
    /// 能動的に再GETし直す（`AppState::enqueue_poll_fetch`、`handlers::notes::queries::
    /// enqueue_stale_poll_fetches`）。既にUpdate(Question)を受理済み（`posts.
    /// poll_update_received`）のNoteは対象外。
    PollFetch { post_id: i64 },
}

/// `JobQueue::dequeue_blocking` が返す、実行対象ジョブとそのメタデータ。
/// `priority`/`attempt` はリトライ時に同じ値で `enqueue_retry` へ引き継ぐために保持する。
/// ジョブハンドラの実行結果エラー。一時的障害（ネットワーク断・タイムアウト等、
/// リトライすれば成功し得る）と恒久的失敗（不正な入力・署名鍵未設定等、リトライしても
/// 同じ結果になる）を区別し、Worker のリトライ戦略（`execute_with_retry`）に反映する。
///
/// 既存の `Result<(), String>` を返すジョブハンドラは `From<String>` で自動的に
/// `Transient` 扱いになる（従来通り最大試行回数までリトライする）ため、この型への
/// 移行は破壊的変更にならない。恒久的失敗と判断できるジョブから順に `Permanent` を
/// 明示的に返すよう移行する。
#[derive(Debug)]
pub enum JobError {
    /// リトライで回復し得る一時的な障害。
    Transient(String),
    /// リトライしても結果が変わらない恒久的な失敗。即座に諦めてよい。
    Permanent(String),
}

impl JobError {
    pub fn message(&self) -> &str {
        match self {
            JobError::Transient(m) | JobError::Permanent(m) => m,
        }
    }

    pub fn is_permanent(&self) -> bool {
        matches!(self, JobError::Permanent(_))
    }
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl From<String> for JobError {
    fn from(s: String) -> Self {
        JobError::Transient(s)
    }
}

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
    async fn enqueue_retry(
        &self,
        job: Job,
        priority: i32,
        attempt: u32,
        delay: Duration,
    ) -> Result<(), String>;

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
            Job::ActorHistorySync {
                ap_uri: Some("https://a.example/users/x".into()),
                at_did: None,
            },
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
            Job::ApDelivery {
                actor_id: 1,
                kind: ApDeliveryKind::DeleteActor,
            },
            Job::InboundActivityProcess {
                raw_activity: "{}".into(),
            },
            Job::AtpRepositoryPublish {
                actor_id: 1,
                commit_type: "create_post".into(),
            },
            Job::BskyVideoPoll { media_file_id: 9 },
            Job::BskyPostCommitDeferred {
                actor_id: 1,
                post_id: 2,
                pending_media_file_id: 3,
            },
            Job::RelayFollowSync {
                relay_id: 1,
                want_follow: true,
            },
            Job::LinkCardEmbedResolve {
                post_id: 2,
                position: 0,
                url: "https://youtube.com/watch?v=x".into(),
            },
            Job::FollowImportProcess { request_id: 1 },
        ];
        for job in jobs {
            let json = serde_json::to_string(&job).expect("serialize");
            let back: Job = serde_json::from_str(&json).expect("deserialize");
            // Job は PartialEq 未実装のため、再シリアライズ結果の一致で確認する
            assert_eq!(json, serde_json::to_string(&back).unwrap());
        }
    }
}

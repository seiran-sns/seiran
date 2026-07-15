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
    /// Announce（リポスト）を配送する。
    Announce { post_id: i64, original_ap_object_id: String },
    /// Undo(Announce)（リポスト取り消し）を配送する。
    UndoAnnounce { announce_post_id: i64, original_ap_object_id: String },
    /// Like/EmojiReact を配送する。`undo_prev` があれば先に旧リアクションの Undo を配送する（切替）。
    Reaction {
        post_id: i64,
        activity_id: String,
        content: String,
        undo_prev: Option<PrevApReaction>,
    },
    /// Undo(Like/EmojiReact)（リアクション取り消し）を配送する。
    UndoReaction { post_id: i64, prev_activity_id: String, content: String },
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
}

#[async_trait]
pub trait JobQueue: Send + Sync {
    /// ジョブを非同期キューに追加します。優先度が低いものから高いものまでサポート。
    async fn enqueue(&self, job: Job, priority: i32) -> Result<(), String>;
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
                    undo_prev: Some(PrevApReaction {
                        activity_id: "https://a.example/activities/r0".into(),
                        content: "❤️".into(),
                    }),
                },
            },
            Job::ApDelivery { actor_id: 1, kind: ApDeliveryKind::DeleteActor },
            Job::InboundActivityProcess { raw_activity: "{}".into() },
            Job::AtpRepositoryPublish { actor_id: 1, commit_type: "create_post".into() },
            Job::BskyVideoPoll { media_file_id: 9 },
        ];
        for job in jobs {
            let json = serde_json::to_string(&job).expect("serialize");
            let back: Job = serde_json::from_str(&json).expect("deserialize");
            // Job は PartialEq 未実装のため、再シリアライズ結果の一致で確認する
            assert_eq!(json, serde_json::to_string(&back).unwrap());
        }
    }
}

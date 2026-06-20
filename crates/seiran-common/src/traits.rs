use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use std::time::Duration;

// ==========================================
// 1. 認証システム (Auth Layer)
// ==========================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtUserInfo {
    pub sub: String,
    pub email: String,
}

#[derive(Debug)]
pub enum AuthError {
    InvalidToken,
    VerificationFailed(String),
    Internal(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::InvalidToken => write!(f, "Invalid token"),
            AuthError::VerificationFailed(msg) => write!(f, "Verification failed: {}", msg),
            AuthError::Internal(msg) => write!(f, "Internal authentication error: {}", msg),
        }
    }
}

impl std::error::Error for AuthError {}

#[async_trait]
pub trait AuthProvider: Send + Sync {
    /// JWT等の認証トークンを検証し、プロバイダ側の一意ユーザー識別子（sub）とメールアドレスを返します。
    async fn verify_token(&self, token: &str) -> Result<ExtUserInfo, AuthError>;
}

// ==========================================
// 2. データベース・共通構造体定義
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Job {
    /// 新規フォローされたアクターの過去ログ（最大300件）を取得・保存する
    ActorHistorySync { ap_uri: Option<String>, at_did: Option<String> },
    
    /// 自住民の新規投稿をAPフォロワーのInboxへ配送する
    OutboundPostDelivery { post_id: i64 },
    
    /// 外部（APのInbox等）から届いたアクティビティを非同期解析・DB保存する
    InboundActivityProcess { raw_activity: String },
    
    /// リモートseiranアクターのハンドシェイク検証、Webfinger解決、アバター等プロキシ
    ActorMetadataResolve { actor_id: i64 },
    
    /// AT Protocolリポジトリのコミットと、リレーへの通知
    AtpRepositoryPublish { actor_id: i64, commit_type: String },
}

#[async_trait]
pub trait JobQueue: Send + Sync {
    /// ジョブを非同期キューに追加します。優先度が低いものから高いものまでサポート。
    async fn enqueue(&self, job: Job, priority: i32) -> Result<(), String>;
}

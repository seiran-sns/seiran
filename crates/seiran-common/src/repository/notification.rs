//! 通知（Misskey 本家の `Notification` エンティティに準拠）の永続化。
//!
//! 以前は WebSocket のプッシュ配信のみでオンメモリ保持（ページ再読み込みで消失、
//! 直近100件までしか遡れない）だった「クイック通知」をDB永続化し、
//! `POST /api/i/notifications`（Misskey 互換）でカーソルページネーション取得できるようにする。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// 通知の種別。Misskey 本家の `type` 値に合わせる
/// （`follow` / `reaction` / `followRequestAccepted` / `mention` / `reply`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Follow,
    Reaction,
    FollowRequestAccepted,
    Mention,
    Reply,
}

impl NotificationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NotificationKind::Follow => "follow",
            NotificationKind::Reaction => "reaction",
            NotificationKind::FollowRequestAccepted => "followRequestAccepted",
            NotificationKind::Mention => "mention",
            NotificationKind::Reply => "reply",
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct NotificationRow {
    pub id: i64,
    pub recipient_actor_id: i64,
    #[sqlx(rename = "type")]
    pub kind: String,
    pub notifier_actor_id: Option<i64>,
    pub note_id: Option<i64>,
    pub reaction: Option<String>,
    /// 通知発生時点で確定していたカスタム絵文字の画像URL（非正規化保存、下記 insert 参照）。
    pub reaction_emoji_url: Option<String>,
    pub is_read: bool,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait NotificationRepository: Send + Sync {
    /// 通知を1件記録する。`id` は呼び出し側で採番済みの snowflake ID
    /// （新しい順ソートに `ORDER BY id DESC` をそのまま使えるようにするため）。
    /// `reaction_emoji_url` は `reaction` がカスタム絵文字（`:shortcode:`）の場合のみ、
    /// 呼び出し時点で解決済みの画像URLを渡す（`reactions` テーブルは
    /// `UNIQUE(post_id, actor_id)` で1人1投稿1リアクションのため、同じアクターが後で
    /// 別の絵文字へ切り替えると過去の行が上書きされ、都度クエリでは解決できなくなるため
    /// 非正規化して保存する）。
    /// `source_uri` はイベントの発生源を特定する一意識別子（ATP Likeの`at_uri`、AP
    /// Reactionの`ap_activity_id`）。渡すと部分ユニークインデックス経由で重複INSERTが
    /// 無視される（firehose/federation-workerの複数起動による複線受信対策。Doc6既知の課題）。
    /// follow系・ローカルリアクションの通知は`None`のままでよい。
    /// `reaction_id` はリアクション通知専用の重複排除トークン（`reactions.id`）。ローカル
    /// リアクション作成時はそのリアクション自身の id を渡し、ATPコミット後に自分自身の
    /// firehose受信で戻ってきた同一リアクションも同じ id を持つため、部分ユニークインデックス
    /// で「ローカル即時通知」と「firehose再受信通知」の二重発生を防げる。follow系・他人発の
    /// リアクション（自分がATPへコミットしていないもの）は`None`のままでよい。
    #[allow(clippy::too_many_arguments)]
    async fn insert(
        &self,
        id: i64,
        recipient_actor_id: i64,
        kind: NotificationKind,
        notifier_actor_id: Option<i64>,
        note_id: Option<i64>,
        reaction: Option<&str>,
        reaction_emoji_url: Option<&str>,
        source_uri: Option<&str>,
        reaction_id: Option<i64>,
    ) -> Result<(), sqlx::Error>;

    /// 自分宛ての通知を新しい順に取得する（カーソルページネーション、`posts` の
    /// タイムライン系クエリと同じ `until_id`/`since_id` 規約）。
    async fn list(
        &self,
        recipient_actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<NotificationRow>, sqlx::Error>;

    /// 自分宛ての未読通知をすべて既読にする（Misskey `markAsRead` 相当）。
    async fn mark_all_read(&self, recipient_actor_id: i64) -> Result<(), sqlx::Error>;
}

pub struct PgNotificationRepository {
    pool: PgPool,
}

impl PgNotificationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl NotificationRepository for PgNotificationRepository {
    async fn insert(
        &self,
        id: i64,
        recipient_actor_id: i64,
        kind: NotificationKind,
        notifier_actor_id: Option<i64>,
        note_id: Option<i64>,
        reaction: Option<&str>,
        reaction_emoji_url: Option<&str>,
        source_uri: Option<&str>,
        reaction_id: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        // ブロック・ミュート関係にある相手からの通知は生成しない（$4=notifier_actor_idが
        // NULL のシステム通知は素通り）。呼び出し元（リアクション作成・inbound Follow/Accept/
        // Reaction・firehose・bsky_follower_poll）はこの1箇所の変更だけで自動的に対象になる。
        // ON CONFLICT はターゲット未指定（DO NOTHING）にして、source_uri・reaction_id
        // どちらの部分ユニークインデックス違反でも無視する（1つのINSERTで両方に対応するため）。
        sqlx::query(
            "INSERT INTO notifications (id, recipient_actor_id, type, notifier_actor_id, note_id, reaction, reaction_emoji_url, source_uri, reaction_id)
             SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9
             WHERE $4::bigint IS NULL OR NOT actor_is_hidden_for_viewer($2, $4)
             ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(recipient_actor_id)
        .bind(kind.as_str())
        .bind(notifier_actor_id)
        .bind(note_id)
        .bind(reaction)
        .bind(reaction_emoji_url)
        .bind(source_uri)
        .bind(reaction_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn list(
        &self,
        recipient_actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<NotificationRow>, sqlx::Error> {
        sqlx::query_as::<_, NotificationRow>(
            "SELECT id, recipient_actor_id, type, notifier_actor_id, note_id, reaction, reaction_emoji_url, is_read, created_at
             FROM notifications
             WHERE recipient_actor_id = $1
               AND ($2::bigint IS NULL OR id < $2)
               AND ($3::bigint IS NULL OR id > $3)
               AND (notifier_actor_id IS NULL OR NOT actor_is_hidden_for_viewer($1, notifier_actor_id))
             ORDER BY id DESC LIMIT $4",
        )
        .bind(recipient_actor_id)
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn mark_all_read(&self, recipient_actor_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE notifications SET is_read = true WHERE recipient_actor_id = $1 AND NOT is_read")
            .bind(recipient_actor_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }
}

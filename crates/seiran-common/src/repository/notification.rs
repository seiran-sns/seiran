//! 通知（Misskey 本家の `Notification` エンティティに準拠）の永続化。
//!
//! 以前は WebSocket のプッシュ配信のみでオンメモリ保持（ページ再読み込みで消失、
//! 直近100件までしか遡れない）だった「クイック通知」をDB永続化し、
//! `POST /api/i/notifications`（Misskey 互換）でカーソルページネーション取得できるようにする。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// 通知の種別。Misskey 本家の `type` 値に合わせる
/// （`follow` / `reaction` / `followRequestAccepted`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Follow,
    Reaction,
    FollowRequestAccepted,
}

impl NotificationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NotificationKind::Follow => "follow",
            NotificationKind::Reaction => "reaction",
            NotificationKind::FollowRequestAccepted => "followRequestAccepted",
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
    pub is_read: bool,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait NotificationRepository: Send + Sync {
    /// 通知を1件記録する。`id` は呼び出し側で採番済みの snowflake ID
    /// （新しい順ソートに `ORDER BY id DESC` をそのまま使えるようにするため）。
    #[allow(clippy::too_many_arguments)]
    async fn insert(
        &self,
        id: i64,
        recipient_actor_id: i64,
        kind: NotificationKind,
        notifier_actor_id: Option<i64>,
        note_id: Option<i64>,
        reaction: Option<&str>,
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
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO notifications (id, recipient_actor_id, type, notifier_actor_id, note_id, reaction)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(recipient_actor_id)
        .bind(kind.as_str())
        .bind(notifier_actor_id)
        .bind(note_id)
        .bind(reaction)
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
            "SELECT id, recipient_actor_id, type, notifier_actor_id, note_id, reaction, is_read, created_at
             FROM notifications
             WHERE recipient_actor_id = $1
               AND ($2::bigint IS NULL OR id < $2)
               AND ($3::bigint IS NULL OR id > $3)
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

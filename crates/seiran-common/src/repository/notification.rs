//! 通知（Misskey 本家の `Notification` エンティティに準拠）の永続化。
//!
//! `POST /api/i/notifications`（Misskey 互換）でカーソルページネーション取得できるようにする。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// 同じ相手からの同じ働きかけ（同じ元ポストのリポスト・同じポストへの同じリアクション）の
/// 通知を、前回の通知からこの時間が経つまで作らない。取り消しと付け直しを繰り返す連打で
/// 通知欄が埋まるのを防ぐ。リモート（Fedi/Bsky）発の連打は送信側のクールダウン
/// （seiran-api の `interaction_cooldown`）が効かないため、受信側でも抑える。
pub const NOTIFICATION_REPEAT_SUPPRESS: chrono::Duration = chrono::Duration::minutes(5);

/// 通知の種別。Misskey 本家の `type` 値に合わせる
/// （`follow` / `reaction` / `followRequestAccepted` / `mention` / `reply` /
/// `repost` / `quote`）。`MoveRefollowed`/`MoveAlreadyFollowing` は Misskey API に無い
/// seiran 独自拡張（ActivityPub Move＝引っ越し受信時の再フォロー通知、`docs/protocols.md`参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Follow,
    Reaction,
    /// フォロー承認制（鍵アカウント）宛てに届いた、承認待ちのフォローリクエスト。
    FollowRequest,
    FollowRequestAccepted,
    Mention,
    Reply,
    Repost,
    Quote,
    MoveRefollowed,
    MoveAlreadyFollowing,
}

impl NotificationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NotificationKind::Follow => "follow",
            NotificationKind::Reaction => "reaction",
            NotificationKind::FollowRequest => "followRequest",
            NotificationKind::FollowRequestAccepted => "followRequestAccepted",
            NotificationKind::Mention => "mention",
            NotificationKind::Reply => "reply",
            NotificationKind::Repost => "repost",
            NotificationKind::Quote => "quote",
            NotificationKind::MoveRefollowed => "moveRefollowed",
            NotificationKind::MoveAlreadyFollowing => "moveAlreadyFollowing",
        }
    }
}

/// 通知一覧の絞り込み条件。各項目は `None` なら絞り込まない。複数項目は AND で作用する。
/// `kinds` は内部種別名（`NotificationKind::as_str`）で、`Some(空)` は何にも一致しない。
#[derive(Debug, Clone, Default)]
pub struct NotificationFilter {
    pub include_kinds: Option<Vec<String>>,
    pub exclude_kinds: Option<Vec<String>>,
    pub notifier_actor_ids: Option<Vec<i64>>,
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
    /// `MoveRefollowed`/`MoveAlreadyFollowing` の移転先アクター（他の種別では常にNULL）。
    pub related_actor_id: Option<i64>,
    /// リポスト通知の元ポスト（`note_id` はラッパー投稿）。列追加前の通知でラッパーが
    /// 残っていなかったものは NULL。
    pub repost_of_post_id: Option<i64>,
    /// リポスト通知のラッパー投稿が取り消し済み（論理削除）または物理削除済みか。
    /// リポスト以外の種別では常に false。
    pub repost_withdrawn: bool,
    pub is_read: bool,
    pub created_at: DateTime<Utc>,
}

/// 作成する通知1件（`NotificationRepository::insert`の入力）。種別ごとに使う項目が異なるため、
/// `NewNotification::new`で必須項目（id・宛先・種別）だけを埋め、残りは構造体更新記法で指定する。
#[derive(Debug, Clone, Copy)]
pub struct NewNotification<'a> {
    pub id: i64,
    pub recipient_actor_id: i64,
    pub kind: NotificationKind,
    pub notifier_actor_id: Option<i64>,
    pub note_id: Option<i64>,
    pub reaction: Option<&'a str>,
    pub reaction_emoji_url: Option<&'a str>,
    /// イベントの発生源を特定する一意識別子（重複INSERT防止、`insert`のコメント参照）。
    pub source_uri: Option<&'a str>,
    /// リアクション通知専用の重複排除トークン（`reactions.id`）。
    pub reaction_id: Option<i64>,
    /// `MoveRefollowed`/`MoveAlreadyFollowing`（引っ越し先アクター）専用の2つ目のアクター参照。
    pub related_actor_id: Option<i64>,
    /// `Repost` 専用。リポストされた元ポスト（`note_id` にはラッパー投稿を渡す）。
    pub repost_of_post_id: Option<i64>,
}

impl NewNotification<'_> {
    pub fn new(id: i64, recipient_actor_id: i64, kind: NotificationKind) -> Self {
        Self {
            id,
            recipient_actor_id,
            kind,
            notifier_actor_id: None,
            note_id: None,
            reaction: None,
            reaction_emoji_url: None,
            source_uri: None,
            reaction_id: None,
            related_actor_id: None,
            repost_of_post_id: None,
        }
    }
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
    /// `related_actor_id` は `MoveRefollowed`/`MoveAlreadyFollowing`（引っ越し先アクター）
    /// 専用の2つ目のアクター参照。他の種別では `None` のままでよい。
    /// `Repost`・`Reaction` は、同じ宛先・同じ通知元の同じ働きかけ（リポストは
    /// `repost_of_post_id`、リアクションは `note_id`+`reaction` が一致）の通知が
    /// `NOTIFICATION_REPEAT_SUPPRESS` 以内にあれば作らない。
    /// 戻り値は実際に通知を作ったか（ブロック・ミュート・重複・連打抑制で作らなかったら
    /// false）。ストリーミングへの通知イベントは true のときだけ流すこと（流すと抑制した
    /// 連打がトーストとして届いてしまう）。
    async fn insert(&self, n: &NewNotification<'_>) -> Result<bool, sqlx::Error>;

    /// 自分宛ての通知を新しい順に取得する（カーソルページネーション、`posts` の
    /// タイムライン系クエリと同じ `until_id`/`since_id` 規約）。
    async fn list(
        &self,
        recipient_actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
        filter: &NotificationFilter,
    ) -> Result<Vec<NotificationRow>, sqlx::Error>;

    /// 自分宛ての未読通知をすべて既読にする（Misskey `markAsRead` 相当）。
    async fn mark_all_read(&self, recipient_actor_id: i64) -> Result<(), sqlx::Error>;
}

/// 連打抑制の対象なら、抑制期間の始まり（これより大きい id の通知を「直近」とみなす）を返す。
/// 通知元の無いシステム通知や、元ポストが分からないリポスト（未解決の Announce）は対象外。
fn repeat_suppress_since(n: &NewNotification<'_>) -> Option<i64> {
    let has_key = match n.kind {
        NotificationKind::Repost => n.repost_of_post_id.is_some(),
        NotificationKind::Reaction => n.note_id.is_some() && n.reaction.is_some(),
        _ => false,
    };
    if n.notifier_actor_id.is_none() || !has_key {
        return None;
    }
    let at = crate::id::snowflake_id_time(n.id) - NOTIFICATION_REPEAT_SUPPRESS;
    Some(crate::id::snowflake_id_floor(at))
}

/// 通知1件の INSERT。`suppress_since` があれば、それより後の同じ働きかけの通知が既に
/// あるとき作らない（`NotificationRepository::insert` 参照）。
async fn insert_notification<'e, E>(
    executor: E,
    n: &NewNotification<'_>,
    suppress_since: Option<i64>,
) -> Result<bool, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    // ブロック・ミュート関係にある相手からの通知は生成しない（$4=notifier_actor_idが
    // NULL のシステム通知は素通り）。呼び出し元（リアクション作成・inbound Follow/Accept/
    // Reaction・firehose・bsky_follower_poll）はこの1箇所の変更だけで自動的に対象になる。
    // ON CONFLICT はターゲット未指定（DO NOTHING）にして、source_uri・reaction_id
    // どちらの部分ユニークインデックス違反でも無視する（1つのINSERTで両方に対応するため）。
    // 連打抑制の照合は宛先の通知を id（Snowflake）の範囲で絞るので
    // idx_notifications_recipient の範囲走査で済む。
    let result = sqlx::query(
        "INSERT INTO notifications (id, recipient_actor_id, type, notifier_actor_id, note_id, reaction, reaction_emoji_url, source_uri, reaction_id, related_actor_id, repost_of_post_id)
         SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11
         WHERE ($4::bigint IS NULL OR NOT actor_is_hidden_for_viewer($2, $4))
           AND ($12::bigint IS NULL OR NOT EXISTS (
               SELECT 1 FROM notifications d
               WHERE d.recipient_actor_id = $2
                 AND d.id > $12
                 AND d.type = $3
                 AND d.notifier_actor_id = $4
                 AND CASE $3
                         WHEN 'repost' THEN d.repost_of_post_id = $11
                         ELSE d.note_id = $5 AND d.reaction = $6
                     END
           ))
         ON CONFLICT DO NOTHING",
    )
    .bind(n.id)
    .bind(n.recipient_actor_id)
    .bind(n.kind.as_str())
    .bind(n.notifier_actor_id)
    .bind(n.note_id)
    .bind(n.reaction)
    .bind(n.reaction_emoji_url)
    .bind(n.source_uri)
    .bind(n.reaction_id)
    .bind(n.related_actor_id)
    .bind(n.repost_of_post_id)
    .bind(suppress_since)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
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
    async fn insert(&self, n: &NewNotification<'_>) -> Result<bool, sqlx::Error> {
        let Some(since_id) = repeat_suppress_since(n) else {
            return insert_notification(&self.pool, n, None).await;
        };
        // 「直近に同じ通知があるか」の判定と INSERT は1文だが、同じ相手からの通知が並行して
        // 届くと（federation-worker の並列処理・firehose）両方が判定をすり抜ける。宛先と
        // 通知元の組でアドバイザリロックを取って直列化する。
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, $2))")
            .bind(format!("notification_repeat:{}", n.recipient_actor_id))
            .bind(n.notifier_actor_id)
            .execute(&mut *tx)
            .await?;
        let inserted = insert_notification(&mut *tx, n, Some(since_id)).await?;
        tx.commit().await?;
        Ok(inserted)
    }

    async fn list(
        &self,
        recipient_actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
        filter: &NotificationFilter,
    ) -> Result<Vec<NotificationRow>, sqlx::Error> {
        sqlx::query_as::<_, NotificationRow>(
            "SELECT n.id, n.recipient_actor_id, n.type, n.notifier_actor_id, n.note_id, n.reaction, n.reaction_emoji_url, n.related_actor_id, n.repost_of_post_id,
                    (n.type = 'repost' AND NOT EXISTS (
                        SELECT 1 FROM posts w WHERE w.id = n.note_id AND w.deleted_at IS NULL
                    )) AS repost_withdrawn,
                    n.is_read, n.created_at
             FROM notifications n
             WHERE recipient_actor_id = $1
               AND id < COALESCE($2::bigint, 9223372036854775807)
               AND id > COALESCE($3::bigint, 0)
               AND (notifier_actor_id IS NULL OR NOT actor_is_hidden_for_viewer($1, notifier_actor_id))
               AND ($5::text[] IS NULL OR type = ANY($5))
               AND ($6::text[] IS NULL OR NOT (type = ANY($6)))
               AND ($7::bigint[] IS NULL OR notifier_actor_id = ANY($7))
             ORDER BY id DESC LIMIT $4",
        )
        .bind(recipient_actor_id)
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .bind(filter.include_kinds.as_deref())
        .bind(filter.exclude_kinds.as_deref())
        .bind(filter.notifier_actor_ids.as_deref())
        .fetch_all(&self.pool)
        .await
    }

    async fn mark_all_read(&self, recipient_actor_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE notifications SET is_read = true WHERE recipient_actor_id = $1 AND NOT is_read",
        )
        .bind(recipient_actor_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }
}

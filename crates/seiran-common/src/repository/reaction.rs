use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

/// プロフィール「投稿」タブの投稿＋リアクション混合フィード用の1行（このアクターが行った
/// リアクション＋対象ポストの投稿者情報）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReactionFeedRow {
    pub id: i64,
    pub content: String,
    pub emoji_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub post_id: i64,
    pub target_actor_id: i64,
    pub target_username: String,
    pub target_domain: String,
    pub target_display_name: Option<String>,
    pub target_actor_type: String,
    pub target_avatar_url: Option<String>,
    pub target_actor_emoji_map: Option<serde_json::Value>,
}

/// リアクションを付けたアクターの表示用情報（ホバーポップオーバーの「誰が付けたか」一覧用）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReactorInfo {
    pub id: i64,
    pub username: String,
    pub domain: String,
    pub actor_type: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    /// Misskey互換API（`POST /api/notes/reactions`）の`id`/`createdAt`用（`reactions.id`/`created_at`）。
    pub reaction_id: i64,
    pub reaction_created_at: chrono::DateTime<chrono::Utc>,
}

/// 記録するリアクション1件（`ReactionRepository::upsert`の入力）。
#[derive(Debug, Clone, Copy)]
pub struct NewReaction<'a> {
    /// 呼び出し側が`generate_snowflake_id(Utc::now())`で事前採番した値
    /// （posts/notifications と同じ名前空間）。
    pub id: i64,
    pub post_id: i64,
    pub actor_id: i64,
    /// `"emoji"`/`"like"`等。
    pub reaction_type: &'a str,
    pub content: &'a str,
    pub ap_activity_id: Option<&'a str>,
    /// `None` なら既存行の値を保持する（非同期の ATP コミット完了を待たずにローカル反映するため）。
    pub at_uri: Option<&'a str>,
    /// カスタム絵文字の画像URL（Unicode絵文字は`None`）。毎回そのまま上書きする。
    pub emoji_url: Option<&'a str>,
}

/// 保存済みリアクション行のうち、取り消し時の配送（AP Undo・ATP Like削除）に必要な値。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredReaction {
    pub content: String,
    pub ap_activity_id: Option<String>,
    pub at_uri: Option<String>,
    pub emoji_url: Option<String>,
}

impl StoredReaction {
    /// ATP側のLikeレコードのrkey（`at_uri`末尾）。
    pub fn atp_rkey(&self) -> Option<&str> {
        self.at_uri.as_deref().and_then(|u| u.rsplit('/').next())
    }
}

/// `ReactionRepository::upsert`の結果。
#[derive(Debug, Clone)]
pub struct ReactionUpsert {
    /// 当該行の`id`（切り替え時も更新後の`id`）。通知の重複排除用トークン
    /// （`notifications.reaction_id`）として使う。
    pub id: i64,
    /// 切り替えで上書きした旧リアクション（新規作成なら`None`）。旧リアクションの AP Undo・
    /// ATP Like 削除に使う。上書きと同じトランザクションで読むため、連打・同時操作でも
    /// 実際に上書きした行の値と一致する。
    pub previous: Option<StoredReaction>,
}

#[async_trait]
pub trait ReactionRepository: Send + Sync {
    /// リアクション（いいね／絵文字リアクション）を記録する。
    /// 1投稿につき1ユーザー1リアクションまで（Misskey 準拠）。同一 (post_id, actor_id) の
    /// 既存リアクションがあれば上書きし（切り替え）、上書き前の値を`previous`で返す。
    /// 切り替え時も `id`/`created_at` を新しい値へ更新する（プロフィール混合フィードで
    /// 「切り替え＝新しいイベント」として時系列先頭に来るべきため）。
    async fn upsert(&self, reaction: &NewReaction<'_>) -> Result<ReactionUpsert, sqlx::Error>;

    /// `ap_activity_id` で特定されるリアクションを取り消す（Undo(Like)/Undo(EmojiReact) 受信時）。
    /// 削除できた場合は `(post_id, actor_id)` を返す（ストリーミング通知の組み立てに使う）。
    async fn delete_by_activity_id(
        &self,
        ap_activity_id: &str,
    ) -> Result<Option<(i64, i64)>, sqlx::Error>;

    /// `at_uri` で特定されるリアクションを取り消す（ATP `app.bsky.feed.like` の delete 受信時）。
    /// 削除できた場合は `(post_id, actor_id)` を返す（ストリーミング通知の組み立てに使う）。
    async fn delete_by_at_uri(&self, at_uri: &str) -> Result<Option<(i64, i64)>, sqlx::Error>;

    /// ローカルユーザーが自分のリアクションを取り消し、削除した行を返す（該当なしなら`None`）。
    /// `content`が`Some`ならその内容のリアクションのみ、`None`なら内容を問わず取り消す
    /// （Misskey互換APIの`notes/reactions/delete`は`noteId`しか受け取らない）。削除と
    /// 取り消し配送用の値の取得を`DELETE ... RETURNING`の1文で行う。
    async fn delete_local(
        &self,
        post_id: i64,
        actor_id: i64,
        content: Option<&str>,
    ) -> Result<Option<StoredReaction>, sqlx::Error>;

    /// 指定ポストの絵文字ごとの件数集計（多い順、`(content, count, emoji_url)`）。
    /// ストリーミング配信ペイロード（`noteUpdated`）の組み立てに使う。閲覧者ごとの `reactedByMe`
    /// は含まない（API 公開用の集計は `fetch_reactions_map` を使う）。`emoji_url` は同一 `content`
    /// の行のうち非NULLな値を代表として1つ返す（異なるドメインの同名カスタム絵文字が
    /// 混在する場合は代表値のみになる簡略仕様）。
    async fn aggregate_for_post(
        &self,
        post_id: i64,
    ) -> Result<Vec<(String, i64, Option<String>)>, sqlx::Error>;

    /// 指定アクターが現在付けているリアクションを `content` 単位で頻度集計する
    /// （絵文字ピッカーの「よく使う絵文字」用、多い順に最大 `limit` 件、`(content, count, emoji_url)`）。
    /// `reactions` は 1投稿1リアクションで切替時に上書きされるため、これは厳密な「過去の使用履歴」
    /// ではなく「現在も付いている自分のリアクション」の集計という近似値になる。
    async fn aggregate_for_actor(
        &self,
        actor_id: i64,
        limit: i64,
    ) -> Result<Vec<(String, i64, Option<String>)>, sqlx::Error>;

    /// 指定 (post_id, content) にリアクションを付けたアクターを新しい順に返す
    /// （リアクションチップのホバーポップオーバー「誰が付けたか」一覧用）。
    /// `viewer_actor_id` がミュート・ブロックしているアクターは除外する
    /// （`actor_is_hidden_for_viewer`）。
    async fn actors_for_reaction(
        &self,
        post_id: i64,
        content: &str,
        viewer_actor_id: Option<i64>,
        limit: i64,
    ) -> Result<Vec<ReactorInfo>, sqlx::Error>;

    /// 指定アクターが行ったリアクションを、対象ポスト・その投稿者情報付きで新しい順に返す
    /// （プロフィール「投稿」タブの投稿＋リアクション混合フィード専用、`until_id`/`since_id` は
    /// `reactions.id` 基準・`timeline_by_actor` と同じカーソル規約）。対象ポストが論理削除済み、
    /// または `viewer_actor_id` から可視でない（`post_is_visible_to` が false）場合は除外する。
    async fn reactions_by_actor_for_feed(
        &self,
        actor_id: i64,
        viewer_actor_id: Option<i64>,
        page: super::Page,
        exclude_direct: bool,
    ) -> Result<Vec<ReactionFeedRow>, sqlx::Error>;
}

pub struct PgReactionRepository {
    pool: PgPool,
}

impl PgReactionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ReactionRepository for PgReactionRepository {
    async fn upsert(&self, r: &NewReaction<'_>) -> Result<ReactionUpsert, sqlx::Error> {
        // 「旧リアクションの読み出し」と「上書き」を別文にすると、連打や同時操作で読み出した
        // 旧値と実際に上書きした行がずれ、リモートに取り消されないLikeが残る。新規挿入を先に
        // 試み、既存行があれば同じトランザクション内で`FOR UPDATE`で読んでから上書きする。
        // 既存行が挿入試行と読み出しの間に削除された場合はやり直す。
        for _ in 0..3 {
            let mut tx = self.pool.begin().await?;
            let inserted: Option<i64> = sqlx::query_scalar(
                "INSERT INTO reactions (id, post_id, actor_id, reaction_type, content, ap_activity_id, at_uri, emoji_url)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (post_id, actor_id) DO NOTHING
                 RETURNING id",
            )
            .bind(r.id)
            .bind(r.post_id)
            .bind(r.actor_id)
            .bind(r.reaction_type)
            .bind(r.content)
            .bind(r.ap_activity_id)
            .bind(r.at_uri)
            .bind(r.emoji_url)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(id) = inserted {
                tx.commit().await?;
                return Ok(ReactionUpsert { id, previous: None });
            }

            let previous: Option<StoredReaction> = sqlx::query_as(
                "SELECT content, ap_activity_id, at_uri, emoji_url FROM reactions
                 WHERE post_id = $1 AND actor_id = $2
                 FOR UPDATE",
            )
            .bind(r.post_id)
            .bind(r.actor_id)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(previous) = previous else {
                continue;
            };
            let id: i64 = sqlx::query_scalar(
                "UPDATE reactions SET
                     id = $3,
                     reaction_type = $4,
                     content = $5,
                     ap_activity_id = $6,
                     at_uri = COALESCE($7, at_uri),
                     emoji_url = $8,
                     created_at = CURRENT_TIMESTAMP
                 WHERE post_id = $1 AND actor_id = $2
                 RETURNING id",
            )
            .bind(r.post_id)
            .bind(r.actor_id)
            .bind(r.id)
            .bind(r.reaction_type)
            .bind(r.content)
            .bind(r.ap_activity_id)
            .bind(r.at_uri)
            .bind(r.emoji_url)
            .fetch_one(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(ReactionUpsert {
                id,
                previous: Some(previous),
            });
        }
        Err(sqlx::Error::RowNotFound)
    }

    async fn delete_by_activity_id(
        &self,
        ap_activity_id: &str,
    ) -> Result<Option<(i64, i64)>, sqlx::Error> {
        let row = sqlx::query(
            "DELETE FROM reactions WHERE ap_activity_id = $1 RETURNING post_id, actor_id",
        )
        .bind(ap_activity_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| (r.get("post_id"), r.get("actor_id"))))
    }

    async fn delete_by_at_uri(&self, at_uri: &str) -> Result<Option<(i64, i64)>, sqlx::Error> {
        let row =
            sqlx::query("DELETE FROM reactions WHERE at_uri = $1 RETURNING post_id, actor_id")
                .bind(at_uri)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|r| (r.get("post_id"), r.get("actor_id"))))
    }

    async fn delete_local(
        &self,
        post_id: i64,
        actor_id: i64,
        content: Option<&str>,
    ) -> Result<Option<StoredReaction>, sqlx::Error> {
        sqlx::query_as(
            "DELETE FROM reactions
             WHERE post_id = $1 AND actor_id = $2 AND ($3::text IS NULL OR content = $3)
             RETURNING content, ap_activity_id, at_uri, emoji_url",
        )
        .bind(post_id)
        .bind(actor_id)
        .bind(content)
        .fetch_optional(&self.pool)
        .await
    }

    async fn aggregate_for_post(
        &self,
        post_id: i64,
    ) -> Result<Vec<(String, i64, Option<String>)>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT content, COUNT(*) AS cnt, MAX(emoji_url) AS emoji_url FROM reactions
             WHERE post_id = $1
             GROUP BY content
             ORDER BY cnt DESC",
        )
        .bind(post_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get("content"), r.get("cnt"), r.get("emoji_url")))
            .collect())
    }

    async fn aggregate_for_actor(
        &self,
        actor_id: i64,
        limit: i64,
    ) -> Result<Vec<(String, i64, Option<String>)>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT content, COUNT(*) AS cnt, MAX(emoji_url) AS emoji_url FROM reactions
             WHERE actor_id = $1
             GROUP BY content
             ORDER BY cnt DESC, MAX(created_at) DESC
             LIMIT $2",
        )
        .bind(actor_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get("content"), r.get("cnt"), r.get("emoji_url")))
            .collect())
    }

    async fn actors_for_reaction(
        &self,
        post_id: i64,
        content: &str,
        viewer_actor_id: Option<i64>,
        limit: i64,
    ) -> Result<Vec<ReactorInfo>, sqlx::Error> {
        sqlx::query_as::<_, ReactorInfo>(
            "SELECT a.id, a.username, a.domain, a.actor_type::text AS actor_type, a.display_name,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    r.id AS reaction_id, r.created_at AS reaction_created_at
             FROM reactions r
             JOIN actors a ON a.id = r.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE r.post_id = $1 AND r.content = $2 AND a.withdrawn_at IS NULL AND a.suspended_at IS NULL
               AND ($4::bigint IS NULL OR NOT actor_is_hidden_for_viewer($4, a.id))
             ORDER BY r.created_at DESC
             LIMIT $3",
        )
        .bind(post_id)
        .bind(content)
        .bind(limit)
        .bind(viewer_actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn reactions_by_actor_for_feed(
        &self,
        actor_id: i64,
        viewer_actor_id: Option<i64>,
        page: super::Page,
        exclude_direct: bool,
    ) -> Result<Vec<ReactionFeedRow>, sqlx::Error> {
        sqlx::query_as::<_, ReactionFeedRow>(
            "SELECT r.id, r.content, r.emoji_url, r.created_at, r.post_id,
                    a.id AS target_actor_id, a.username AS target_username, a.domain AS target_domain,
                    a.display_name AS target_display_name, a.actor_type::text AS target_actor_type,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS target_avatar_url,
                    a.emoji_map AS target_actor_emoji_map
             FROM reactions r
             JOIN posts p ON p.id = r.post_id
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE r.actor_id = $1
               AND p.deleted_at IS NULL
               AND ($2::bigint IS NULL OR r.id < $2)
               AND ($3::bigint IS NULL OR r.id > $3)
               AND ($4::bigint IS NULL OR $4 = $1 OR NOT actor_is_hidden_for_viewer($4, $1))
               AND post_is_visible_to($4, p.actor_id, p.visibility::text, p.id, $5)
             ORDER BY r.id DESC
             LIMIT $6",
        )
        .bind(actor_id)
        .bind(page.until_id)
        .bind(page.since_id)
        .bind(viewer_actor_id)
        .bind(exclude_direct)
        .bind(page.limit)
        .fetch_all(&self.pool)
        .await
    }
}

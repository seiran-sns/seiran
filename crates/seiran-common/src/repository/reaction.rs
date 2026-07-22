use async_trait::async_trait;
use sqlx::{PgPool, Row};

#[async_trait]
pub trait ReactionRepository: Send + Sync {
    /// リアクション（いいね／絵文字リアクション）を記録する。
    /// 1投稿につき1ユーザー1リアクションまで（Misskey 準拠）。同一 (post_id, actor_id) の
    /// 既存リアクションがあれば上書きする（切り替え）。`ap_activity_id` の重複は無視する。
    /// `at_uri`（ATP 連携）は `None` を渡すと既存値を保持する（非同期の ATP コミット完了を
    /// 待たずにローカル反映するため。`at_uri` の設定自体は `AtpCommitService::commit_like` や
    /// INBOUND の Like 受信経路が別途行う）。
    /// `emoji_url` はカスタム絵文字（`:shortcode:`）の画像 URL（ローカル送信は
    /// `custom_emojis` から解決した URL、Fedi 受信は activity の `tag` から解決した URL）。
    /// Unicode 絵文字リアクションでは `None`。`at_uri` と異なり毎回そのまま上書きする
    /// （都度解決した URL か `None` を渡すため、旧値を保持する必要が無い）。
    /// 戻り値は当該行の `id`（切り替え時も同じ (post_id, actor_id) の既存行の id）。
    /// 通知の重複排除用トークン（`notifications.reaction_id`）として使う。
    #[allow(clippy::too_many_arguments)]
    async fn insert(
        &self,
        post_id: i64,
        actor_id: i64,
        reaction_type: &str,
        content: &str,
        ap_activity_id: Option<&str>,
        at_uri: Option<&str>,
        emoji_url: Option<&str>,
    ) -> Result<i64, sqlx::Error>;

    /// `ap_activity_id` で特定されるリアクションを取り消す（Undo(Like)/Undo(EmojiReact) 受信時）。
    /// 削除できた場合は `(post_id, actor_id)` を返す（ストリーミング通知の組み立てに使う）。
    async fn delete_by_activity_id(
        &self,
        ap_activity_id: &str,
    ) -> Result<Option<(i64, i64)>, sqlx::Error>;

    /// `at_uri` で特定されるリアクションを取り消す（ATP `app.bsky.feed.like` の delete 受信時）。
    /// 削除できた場合は `(post_id, actor_id)` を返す（ストリーミング通知の組み立てに使う）。
    async fn delete_by_at_uri(&self, at_uri: &str) -> Result<Option<(i64, i64)>, sqlx::Error>;

    /// ローカルユーザーが自分の (post_id, actor_id, content) の組み合わせでリアクションを取り消す。
    /// 返り値は削除行数（0 なら該当リアクションなし）。
    async fn delete_local(&self, post_id: i64, actor_id: i64, content: &str) -> Result<u64, sqlx::Error>;

    /// 指定 (post_id, actor_id) の現在のリアクション行から `content` / `ap_activity_id` / `at_uri`
    /// / `emoji_url` を取得する。切替・取消の際、事前に「削除すべき旧リアクション（AP の Undo
    /// 対象、ATP の削除対象 rkey）」を退避するために使う。
    async fn find_current(
        &self,
        post_id: i64,
        actor_id: i64,
    ) -> Result<Option<(String, Option<String>, Option<String>, Option<String>)>, sqlx::Error>;

    /// 指定ポストの絵文字ごとの件数集計（多い順、`(content, count, emoji_url)`）。
    /// ストリーミング配信ペイロード（`noteUpdated`）の組み立てに使う。閲覧者ごとの `reactedByMe`
    /// は含まない（API 公開用の集計は `fetch_reactions_map` を使う）。`emoji_url` は同一 `content`
    /// の行のうち非NULLな値を代表として1つ返す（異なるドメインの同名カスタム絵文字が
    /// 混在する場合は代表値のみになる簡略仕様）。
    async fn aggregate_for_post(&self, post_id: i64) -> Result<Vec<(String, i64, Option<String>)>, sqlx::Error>;

    /// 指定アクターが現在付けているリアクションを `content` 単位で頻度集計する
    /// （絵文字ピッカーの「よく使う絵文字」用、多い順に最大 `limit` 件、`(content, count, emoji_url)`）。
    /// `reactions` は 1投稿1リアクションで切替時に上書きされるため、これは厳密な「過去の使用履歴」
    /// ではなく「現在も付いている自分のリアクション」の集計という近似値になる。
    async fn aggregate_for_actor(&self, actor_id: i64, limit: i64) -> Result<Vec<(String, i64, Option<String>)>, sqlx::Error>;
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
    async fn insert(
        &self,
        post_id: i64,
        actor_id: i64,
        reaction_type: &str,
        content: &str,
        ap_activity_id: Option<&str>,
        at_uri: Option<&str>,
        emoji_url: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let row = sqlx::query(
            "INSERT INTO reactions (post_id, actor_id, reaction_type, content, ap_activity_id, at_uri, emoji_url)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (post_id, actor_id) DO UPDATE SET
                 reaction_type = EXCLUDED.reaction_type,
                 content = EXCLUDED.content,
                 ap_activity_id = EXCLUDED.ap_activity_id,
                 at_uri = COALESCE(EXCLUDED.at_uri, reactions.at_uri),
                 emoji_url = EXCLUDED.emoji_url,
                 created_at = CURRENT_TIMESTAMP
             RETURNING id",
        )
        .bind(post_id)
        .bind(actor_id)
        .bind(reaction_type)
        .bind(content)
        .bind(ap_activity_id)
        .bind(at_uri)
        .bind(emoji_url)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get::<i64, _>("id"))
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
        let row = sqlx::query(
            "DELETE FROM reactions WHERE at_uri = $1 RETURNING post_id, actor_id",
        )
        .bind(at_uri)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| (r.get("post_id"), r.get("actor_id"))))
    }

    async fn find_current(
        &self,
        post_id: i64,
        actor_id: i64,
    ) -> Result<Option<(String, Option<String>, Option<String>, Option<String>)>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT content, ap_activity_id, at_uri, emoji_url FROM reactions WHERE post_id = $1 AND actor_id = $2",
        )
        .bind(post_id)
        .bind(actor_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| (r.get("content"), r.get("ap_activity_id"), r.get("at_uri"), r.get("emoji_url"))))
    }

    async fn delete_local(&self, post_id: i64, actor_id: i64, content: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "DELETE FROM reactions WHERE post_id = $1 AND actor_id = $2 AND content = $3",
        )
        .bind(post_id)
        .bind(actor_id)
        .bind(content)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn aggregate_for_post(&self, post_id: i64) -> Result<Vec<(String, i64, Option<String>)>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT content, COUNT(*) AS cnt, MAX(emoji_url) AS emoji_url FROM reactions
             WHERE post_id = $1
             GROUP BY content
             ORDER BY cnt DESC",
        )
        .bind(post_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| (r.get("content"), r.get("cnt"), r.get("emoji_url"))).collect())
    }

    async fn aggregate_for_actor(&self, actor_id: i64, limit: i64) -> Result<Vec<(String, i64, Option<String>)>, sqlx::Error> {
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
        Ok(rows.into_iter().map(|r| (r.get("content"), r.get("cnt"), r.get("emoji_url"))).collect())
    }
}

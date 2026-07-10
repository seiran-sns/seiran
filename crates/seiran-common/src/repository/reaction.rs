use async_trait::async_trait;
use sqlx::{PgPool, Row};

#[async_trait]
pub trait ReactionRepository: Send + Sync {
    /// リアクション（いいね／絵文字リアクション）を記録する。
    /// 1投稿につき1ユーザー1リアクションまで（Misskey 準拠）。同一 (post_id, actor_id) の
    /// 既存リアクションがあれば上書きする（切り替え）。`ap_activity_id` の重複は無視する。
    async fn insert(
        &self,
        post_id: i64,
        actor_id: i64,
        reaction_type: &str,
        content: &str,
        ap_activity_id: Option<&str>,
    ) -> Result<(), sqlx::Error>;

    /// `ap_activity_id` で特定されるリアクションを取り消す（Undo(Like)/Undo(EmojiReact) 受信時）。
    /// 削除できた場合は `(post_id, actor_id)` を返す（ストリーミング通知の組み立てに使う）。
    async fn delete_by_activity_id(
        &self,
        ap_activity_id: &str,
    ) -> Result<Option<(i64, i64)>, sqlx::Error>;

    /// ローカルユーザーが自分の (post_id, actor_id, content) の組み合わせでリアクションを取り消す。
    /// 返り値は削除行数（0 なら該当リアクションなし）。
    async fn delete_local(&self, post_id: i64, actor_id: i64, content: &str) -> Result<u64, sqlx::Error>;

    /// 指定ポストの絵文字ごとの件数集計（多い順）。ストリーミング配信ペイロード（`noteUpdated`）の
    /// 組み立てに使う。閲覧者ごとの `reactedByMe` は含まない（API 公開用の集計は `fetch_reactions_map` を使う）。
    async fn aggregate_for_post(&self, post_id: i64) -> Result<Vec<(String, i64)>, sqlx::Error>;
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
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO reactions (post_id, actor_id, reaction_type, content, ap_activity_id)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (post_id, actor_id) DO UPDATE SET
                 reaction_type = EXCLUDED.reaction_type,
                 content = EXCLUDED.content,
                 ap_activity_id = EXCLUDED.ap_activity_id,
                 created_at = CURRENT_TIMESTAMP",
        )
        .bind(post_id)
        .bind(actor_id)
        .bind(reaction_type)
        .bind(content)
        .bind(ap_activity_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
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

    async fn aggregate_for_post(&self, post_id: i64) -> Result<Vec<(String, i64)>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT content, COUNT(*) AS cnt FROM reactions
             WHERE post_id = $1
             GROUP BY content
             ORDER BY cnt DESC",
        )
        .bind(post_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| (r.get("content"), r.get("cnt"))).collect())
    }
}

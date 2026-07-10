use async_trait::async_trait;
use sqlx::PgPool;

#[async_trait]
pub trait ReactionRepository: Send + Sync {
    /// リアクション（いいね／絵文字リアクション）を記録する。
    /// `post_id + actor_id + content` または `ap_activity_id` の重複は無視する。
    async fn insert(
        &self,
        post_id: i64,
        actor_id: i64,
        reaction_type: &str,
        content: &str,
        ap_activity_id: Option<&str>,
    ) -> Result<(), sqlx::Error>;

    /// `ap_activity_id` で特定されるリアクションを取り消す（Undo(Like)/Undo(EmojiReact) 受信時）。
    /// 返り値は削除行数。
    async fn delete_by_activity_id(&self, ap_activity_id: &str) -> Result<u64, sqlx::Error>;
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
             ON CONFLICT DO NOTHING",
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

    async fn delete_by_activity_id(&self, ap_activity_id: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM reactions WHERE ap_activity_id = $1")
            .bind(ap_activity_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}

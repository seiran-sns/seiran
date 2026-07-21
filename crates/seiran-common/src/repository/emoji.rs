use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// `custom_emojis` テーブルの 1 行。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EmojiRow {
    pub id: i64,
    pub shortcode: String,
    pub media_file_id: i64,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait EmojiRepository: Send + Sync {
    /// 全カスタム絵文字を ID 昇順で返す。
    async fn list_all(&self) -> Result<Vec<EmojiRow>, sqlx::Error>;

    /// 新規絵文字を登録する。shortcode の一意制約違反はそのまま `sqlx::Error` を返す
    /// （呼び出し側で `23505` を判定して `ApiError::Conflict` に変換する）。
    async fn insert(
        &self,
        id: i64,
        shortcode: &str,
        media_file_id: i64,
        category: Option<&str>,
        tags: &[String],
    ) -> Result<EmojiRow, sqlx::Error>;

    /// category / tags を更新する（`None` は現在値を保持）。
    async fn update(
        &self,
        id: i64,
        category: Option<&str>,
        tags: Option<&[String]>,
    ) -> Result<Option<EmojiRow>, sqlx::Error>;

    /// 削除する。削除できたら true。
    async fn delete(&self, id: i64) -> Result<bool, sqlx::Error>;

    /// shortcode が既に登録済みかを返す（Misskey絵文字インポートの重複スキップ用）。
    async fn exists_by_shortcode(&self, shortcode: &str) -> Result<bool, sqlx::Error>;

    /// shortcode が未登録の場合のみ挿入する（`ON CONFLICT DO NOTHING`）。挿入できたら true。
    #[allow(clippy::too_many_arguments)]
    async fn insert_if_absent(
        &self,
        id: i64,
        shortcode: &str,
        media_file_id: i64,
        category: Option<&str>,
        tags: &[String],
        license: Option<&str>,
    ) -> Result<bool, sqlx::Error>;
}

pub struct PgEmojiRepository {
    pool: PgPool,
}

impl PgEmojiRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl EmojiRepository for PgEmojiRepository {
    async fn list_all(&self) -> Result<Vec<EmojiRow>, sqlx::Error> {
        sqlx::query_as::<_, EmojiRow>(
            "SELECT id, shortcode, media_file_id, category, tags, created_at
             FROM custom_emojis
             ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
    }

    async fn insert(
        &self,
        id: i64,
        shortcode: &str,
        media_file_id: i64,
        category: Option<&str>,
        tags: &[String],
    ) -> Result<EmojiRow, sqlx::Error> {
        sqlx::query_as::<_, EmojiRow>(
            "INSERT INTO custom_emojis (id, shortcode, media_file_id, category, tags)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id, shortcode, media_file_id, category, tags, created_at",
        )
        .bind(id)
        .bind(shortcode)
        .bind(media_file_id)
        .bind(category)
        .bind(tags)
        .fetch_one(&self.pool)
        .await
    }

    async fn update(
        &self,
        id: i64,
        category: Option<&str>,
        tags: Option<&[String]>,
    ) -> Result<Option<EmojiRow>, sqlx::Error> {
        sqlx::query_as::<_, EmojiRow>(
            "UPDATE custom_emojis
             SET category = COALESCE($2, category),
                 tags     = COALESCE($3, tags)
             WHERE id = $1
             RETURNING id, shortcode, media_file_id, category, tags, created_at",
        )
        .bind(id)
        .bind(category)
        .bind(tags)
        .fetch_optional(&self.pool)
        .await
    }

    async fn delete(&self, id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM custom_emojis WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn exists_by_shortcode(&self, shortcode: &str) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM custom_emojis WHERE shortcode = $1)")
            .bind(shortcode)
            .fetch_one(&self.pool)
            .await
    }

    async fn insert_if_absent(
        &self,
        id: i64,
        shortcode: &str,
        media_file_id: i64,
        category: Option<&str>,
        tags: &[String],
        license: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "INSERT INTO custom_emojis (id, shortcode, media_file_id, category, tags, license)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (shortcode) DO NOTHING",
        )
        .bind(id)
        .bind(shortcode)
        .bind(media_file_id)
        .bind(category)
        .bind(tags)
        .bind(license)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }
}

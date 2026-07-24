use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// `:shortcode:` 形式かどうかを判定し、妥当ならコロンを除いた shortcode を返す。
/// 許可する文字種は admin 絵文字登録（`shortcode`）バリデーションと揃える（英数字・アンダースコアのみ）。
/// ローカル送信（`handlers/notes/validation.rs`）と ATP 自己firehose再受信
/// （`seiran-atp-repo::firehose::handle_inbound_like_create`）の両方で、`reactions.content`
/// からカスタム絵文字の実在確認・画像URL解決が必要かどうかを判定するために使う共通ロジック。
pub fn parse_custom_emoji_shortcode(s: &str) -> Option<&str> {
    let inner = s.strip_prefix(':')?.strip_suffix(':')?;
    if inner.is_empty() || !inner.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some(inner)
}

/// `custom_emojis` テーブルの 1 行。
/// `url` は `list_all` のみ `media_files`/`storage_providers` を JOIN して解決する
/// （admin 一覧の画像プレビュー用）。`insert`/`update` は JOIN しないため常に `None`。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EmojiRow {
    pub id: i64,
    pub shortcode: String,
    pub media_file_id: i64,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub license: Option<String>,
    pub created_at: DateTime<Utc>,
    pub url: Option<String>,
}

#[async_trait]
pub trait EmojiRepository: Send + Sync {
    /// 全カスタム絵文字を ID 昇順で返す。
    async fn list_all(&self) -> Result<Vec<EmojiRow>, sqlx::Error>;

    /// 新規絵文字を登録する。shortcode の一意制約違反はそのまま `sqlx::Error` を返す
    /// （呼び出し側で `23505` を判定して `ApiError::Conflict` に変換する）。
    #[allow(clippy::too_many_arguments)]
    async fn insert(
        &self,
        id: i64,
        shortcode: &str,
        media_file_id: i64,
        category: Option<&str>,
        tags: &[String],
        license: Option<&str>,
    ) -> Result<EmojiRow, sqlx::Error>;

    /// category / tags / license を更新する（`None` は現在値を保持）。
    async fn update(
        &self,
        id: i64,
        category: Option<&str>,
        tags: Option<&[String]>,
        license: Option<&str>,
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

    /// shortcode から画像 URL を解決する（カスタム絵文字リアクション用）。未登録なら `None`。
    async fn find_url_by_shortcode(&self, shortcode: &str) -> Result<Option<String>, sqlx::Error>;
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
            "SELECT ce.id, ce.shortcode, ce.media_file_id, ce.category, ce.tags, ce.license, ce.created_at,
                    rtrim(sp.public_url, '/') || '/' || mf.storage_key AS url
             FROM custom_emojis ce
             JOIN media_files mf ON mf.id = ce.media_file_id
             JOIN storage_providers sp ON sp.id = mf.storage_provider_id
             ORDER BY ce.id",
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
        license: Option<&str>,
    ) -> Result<EmojiRow, sqlx::Error> {
        sqlx::query_as::<_, EmojiRow>(
            "INSERT INTO custom_emojis (id, shortcode, media_file_id, category, tags, license)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, shortcode, media_file_id, category, tags, license, created_at, NULL::text AS url",
        )
        .bind(id)
        .bind(shortcode)
        .bind(media_file_id)
        .bind(category)
        .bind(tags)
        .bind(license)
        .fetch_one(&self.pool)
        .await
    }

    async fn update(
        &self,
        id: i64,
        category: Option<&str>,
        tags: Option<&[String]>,
        license: Option<&str>,
    ) -> Result<Option<EmojiRow>, sqlx::Error> {
        sqlx::query_as::<_, EmojiRow>(
            "UPDATE custom_emojis
             SET category = COALESCE($2, category),
                 tags     = COALESCE($3, tags),
                 license  = COALESCE($4, license)
             WHERE id = $1
             RETURNING id, shortcode, media_file_id, category, tags, license, created_at, NULL::text AS url",
        )
        .bind(id)
        .bind(category)
        .bind(tags)
        .bind(license)
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

    async fn find_url_by_shortcode(&self, shortcode: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT rtrim(sp.public_url, '/') || '/' || mf.storage_key
             FROM custom_emojis ce
             JOIN media_files mf ON mf.id = ce.media_file_id
             JOIN storage_providers sp ON sp.id = mf.storage_provider_id
             WHERE ce.shortcode = $1",
        )
        .bind(shortcode)
        .fetch_optional(&self.pool)
        .await
    }
}

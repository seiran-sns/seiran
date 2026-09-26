use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MediaFile {
    pub id: i64,
    pub storage_provider_id: i64,
    pub sha256: String,
    pub blurhash: Option<String>,
    pub size: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub mime_type: String,
    pub storage_key: String,
    pub duration_ms: Option<i32>,
    pub thumbnail_key: Option<String>,
    pub uploaded_by_actor_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    /// アニメーション画像（GIF/APNG/WebPアニメ）由来かどうか。`storage::image::ImagePipeline`
    /// が `AnimatedPassthrough` を返した場合のみ `true`（静止画は再エンコードでアニメでない
    /// フォーマットへ確定するため常に `false`）。Bsky embed選択（#227）で「静止画」と
    /// 「アニメGIF」を区別するために使う。
    pub is_animated_image: bool,
    /// 最後に「アップロードされた」時刻（新規保存、または既存sha256一致による重複排除
    /// ヒット時の両方で更新される）。`run_media_gc` の生存判定はこちらを見る。`created_at`
    /// だけを見ていると、古い孤児ファイルが別ユーザーの再アップロードでID再利用された直後に
    /// GCが誤って削除してしまうレースがあった。
    pub last_uploaded_at: DateTime<Utc>,
}

/// `resolve_public_by_sha256` の戻り値。
pub struct ResolvedMediaFile {
    pub url: String,
    pub mime_type: String,
    pub is_animated_image: bool,
}

pub struct CreateMediaFile {
    pub id: i64,
    pub storage_provider_id: i64,
    pub sha256: String,
    pub blurhash: Option<String>,
    pub size: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub mime_type: String,
    pub storage_key: String,
    pub duration_ms: Option<i32>,
    pub thumbnail_key: Option<String>,
    pub uploaded_by_actor_id: Option<i64>,
    pub is_animated_image: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum MediaFileError {
    #[error("DB エラー: {0}")]
    Db(#[from] sqlx::Error),
}

const SELECT_COLS: &str =
    "id, storage_provider_id, sha256, blurhash, size, width, height, mime_type, storage_key, duration_ms, thumbnail_key, uploaded_by_actor_id, created_at, is_animated_image, last_uploaded_at";

#[async_trait]
pub trait MediaFileRepository: Send + Sync {
    /// SHA-256 と blurhash が一致するファイルを返す（重複排除用）。
    async fn find_by_sha256_and_blurhash(
        &self,
        sha256: &str,
        blurhash: &str,
    ) -> Result<Option<MediaFile>, MediaFileError>;

    /// SHA-256 のみで一致するファイルを返す（重複排除用。`blurhash` が概念上
    /// 存在しない音声ファイル向け。`blurhash IS NULL` の行のみが対象）。
    async fn find_by_sha256(&self, sha256: &str) -> Result<Option<MediaFile>, MediaFileError>;

    async fn find_by_id(&self, id: i64) -> Result<Option<MediaFile>, MediaFileError>;

    /// SHA-256 から公開URL・MIMEタイプ・アニメーション画像かどうかを解決する
    /// （`/api/site-icon/:sha256/:size` 用。`storage_providers.public_url` と
    /// `media_files.storage_key` を結合して公開URLを組み立てる）。
    async fn resolve_public_by_sha256(
        &self,
        sha256: &str,
    ) -> Result<Option<ResolvedMediaFile>, MediaFileError>;

    /// `(sha256, blurhash)` が既存ならその行の `last_uploaded_at` を更新して返し
    /// （戻り値の `bool` は `false`）、無ければ新規に挿入する（`bool` は `true`）。
    /// 単一の `INSERT ... ON CONFLICT ... DO UPDATE` 文で完結するアトミックな
    /// upsert（`media_files_sha256_blurhash_key` は `NULLS NOT DISTINCT` なので
    /// `blurhash = NULL` の行同士も衝突として扱われる）。
    ///
    /// 呼び出し元がアップロード直前に重複排除の事前チェック（`find_by_sha256*`）を
    /// 行っていても、その確認からこの呼び出しまでの間に別のリクエストが同じ内容を
    /// 先に登録してしまう競合は起こりうる。このメソッドがその競合を安全に吸収する
    /// 最終防衛線であり、「事前チェックで無かったのでアップロード直後に新規登録した
    /// つもりが、実は既存行に統合された」場合でも `last_uploaded_at` の更新漏れが
    /// 起きない（更新漏れがあると、登録した直後に孤立ファイルGCへ拾われて削除される
    /// レースになる）。
    async fn upsert(&self, req: CreateMediaFile) -> Result<(MediaFile, bool), MediaFileError>;

    /// 重複排除で既存行を再利用した際に `last_uploaded_at` を現在時刻へ更新する
    /// （`run_media_gc` の生存判定をこの再利用も「使われている」とみなせるようにする）。
    /// `upsert` と異なり新規挿入は行わない軽量パス（アップロード事前チェックの
    /// SELECTが既存行を見つけ、S3への保存自体をスキップできる場合に使う）。
    async fn touch_last_uploaded_at(&self, id: i64) -> Result<(), MediaFileError>;
}

pub struct PgMediaFileRepository {
    pool: PgPool,
}

impl PgMediaFileRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MediaFileRepository for PgMediaFileRepository {
    async fn find_by_sha256_and_blurhash(
        &self,
        sha256: &str,
        blurhash: &str,
    ) -> Result<Option<MediaFile>, MediaFileError> {
        let row = sqlx::query_as::<_, MediaFile>(&format!(
            "SELECT {SELECT_COLS} FROM media_files WHERE sha256 = $1 AND blurhash = $2 LIMIT 1"
        ))
        .bind(sha256)
        .bind(blurhash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn find_by_sha256(&self, sha256: &str) -> Result<Option<MediaFile>, MediaFileError> {
        let row = sqlx::query_as::<_, MediaFile>(&format!(
            "SELECT {SELECT_COLS} FROM media_files WHERE sha256 = $1 AND blurhash IS NULL LIMIT 1"
        ))
        .bind(sha256)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn find_by_id(&self, id: i64) -> Result<Option<MediaFile>, MediaFileError> {
        let row = sqlx::query_as::<_, MediaFile>(&format!(
            "SELECT {SELECT_COLS} FROM media_files WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn resolve_public_by_sha256(
        &self,
        sha256: &str,
    ) -> Result<Option<ResolvedMediaFile>, MediaFileError> {
        let row: Option<(String, String, String, bool)> = sqlx::query_as(
            "SELECT sp.public_url, mf.storage_key, mf.mime_type, mf.is_animated_image \
             FROM media_files mf \
             JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
             WHERE mf.sha256 = $1 \
             ORDER BY mf.id LIMIT 1",
        )
        .bind(sha256)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(public_url, storage_key, mime_type, is_animated_image)| ResolvedMediaFile {
                url: format!("{}/{}", public_url.trim_end_matches('/'), storage_key),
                mime_type,
                is_animated_image,
            },
        ))
    }

    async fn upsert(&self, req: CreateMediaFile) -> Result<(MediaFile, bool), MediaFileError> {
        let row = sqlx::query(&format!(
            "INSERT INTO media_files \
             (id, storage_provider_id, sha256, blurhash, size, width, height, mime_type, storage_key, duration_ms, thumbnail_key, uploaded_by_actor_id, is_animated_image) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) \
             ON CONFLICT (sha256, blurhash) DO UPDATE SET last_uploaded_at = now() \
             RETURNING {SELECT_COLS}, (xmax = 0) AS inserted"
        ))
        .bind(req.id)
        .bind(req.storage_provider_id)
        .bind(req.sha256)
        .bind(req.blurhash)
        .bind(req.size)
        .bind(req.width)
        .bind(req.height)
        .bind(req.mime_type)
        .bind(req.storage_key)
        .bind(req.duration_ms)
        .bind(req.thumbnail_key)
        .bind(req.uploaded_by_actor_id)
        .bind(req.is_animated_image)
        .fetch_one(&self.pool)
        .await?;

        let inserted: bool = row.try_get("inserted")?;
        let file = MediaFile {
            id: row.try_get("id")?,
            storage_provider_id: row.try_get("storage_provider_id")?,
            sha256: row.try_get("sha256")?,
            blurhash: row.try_get("blurhash")?,
            size: row.try_get("size")?,
            width: row.try_get("width")?,
            height: row.try_get("height")?,
            mime_type: row.try_get("mime_type")?,
            storage_key: row.try_get("storage_key")?,
            duration_ms: row.try_get("duration_ms")?,
            thumbnail_key: row.try_get("thumbnail_key")?,
            uploaded_by_actor_id: row.try_get("uploaded_by_actor_id")?,
            created_at: row.try_get("created_at")?,
            is_animated_image: row.try_get("is_animated_image")?,
            last_uploaded_at: row.try_get("last_uploaded_at")?,
        };
        Ok((file, inserted))
    }

    async fn touch_last_uploaded_at(&self, id: i64) -> Result<(), MediaFileError> {
        sqlx::query("UPDATE media_files SET last_uploaded_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

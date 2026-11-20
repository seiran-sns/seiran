//! 画像の重複排除・S3アップロード・DB登録。
//!
//! `storage::prepare_image()` が返す `ImagePipeline` には「Exif整理済み無劣化オリジナル」
//! と「Orientation適用・リサイズ・WebP化」の最大2候補が含まれる。ここでは各候補を
//! 順にDB重複チェックし、どちらも未登録ならバイトサイズが小さい方を採用して保存する
//! （ユーザーの画像を不要に劣化させないため）。API のアップロードと、ジョブ
//! （絵文字インポート）の両方が使う。

use chrono::Utc;
use uuid::Uuid;

use crate::generate_snowflake_id;
use crate::repository::{
    CreateMediaFile, MediaFile, MediaFileRepository, StorageProvider, StorageProviderRepository,
};
use crate::storage::{
    select_provider, ExifSanitizedImage, ImagePipeline, ProcessedImage, S3StorageClient,
    SelectorError,
};

#[derive(Debug, thiserror::Error)]
pub enum MediaStoreError {
    #[error(transparent)]
    Selector(#[from] SelectorError),
    /// プロバイダー確定後の再確認で容量上限を超えていた。
    #[error("ストレージの容量上限に達しています")]
    QuotaExceeded,
    #[error("ストレージへの保存に失敗しました: {0}")]
    Storage(String),
    #[error("DB エラー: {0}")]
    Db(String),
}

pub struct UploadOutcome {
    pub record: MediaFile,
    pub is_reused: bool,
}

/// 保存に使うリポジトリの組。
#[derive(Clone, Copy)]
pub struct MediaStore<'a> {
    pub media_files: &'a dyn MediaFileRepository,
    pub storage_providers: &'a dyn StorageProviderRepository,
}

struct NewMedia {
    sha256: String,
    blurhash: String,
    size: i64,
    width: u32,
    height: u32,
    mime_type: String,
    data: Vec<u8>,
    is_animated_image: bool,
}

impl From<ProcessedImage> for NewMedia {
    fn from(p: ProcessedImage) -> Self {
        Self {
            sha256: p.sha256,
            blurhash: p.blurhash,
            size: p.size,
            width: p.width,
            height: p.height,
            mime_type: p.mime_type,
            data: p.data,
            is_animated_image: false,
        }
    }
}

impl From<ExifSanitizedImage> for NewMedia {
    fn from(o: ExifSanitizedImage) -> Self {
        Self {
            sha256: o.sha256,
            blurhash: o.blurhash,
            size: o.size,
            width: o.width,
            height: o.height,
            mime_type: o.mime_type,
            data: o.data,
            is_animated_image: false,
        }
    }
}

/// プロバイダー確定後、PUT 前にもう一度容量を確かめる（選択から PUT までの間に他の
/// アップロードが容量を埋めていることがあるため）。
pub async fn check_quota(
    storage_providers: &dyn StorageProviderRepository,
    provider: &StorageProvider,
    upload_size: i64,
) -> Result<(), MediaStoreError> {
    if let Some(cap_mb) = provider.capacity_mb {
        let used_bytes = storage_providers
            .get_used_bytes(provider.id)
            .await
            .map_err(|e| MediaStoreError::Db(e.to_string()))?;
        if cap_mb * 1024 * 1024 < used_bytes + upload_size {
            return Err(MediaStoreError::QuotaExceeded);
        }
    }
    Ok(())
}

impl MediaStore<'_> {
    async fn find_existing(
        &self,
        sha256: &str,
        blurhash: &str,
    ) -> Result<Option<MediaFile>, MediaStoreError> {
        let existing = self
            .media_files
            .find_by_sha256_and_blurhash(sha256, blurhash)
            .await
            .map_err(|e| MediaStoreError::Db(e.to_string()))?;
        if let Some(existing) = &existing {
            self.media_files
                .touch_last_uploaded_at(existing.id)
                .await
                .map_err(|e| MediaStoreError::Db(e.to_string()))?;
        }
        Ok(existing)
    }

    /// 事前チェック（`find_existing`）が「無い」と判定した場合の保存処理。
    /// 実際のDB登録は `upsert` に委ねるため、事前チェックからここに来るまでの間に
    /// 別のリクエストが同じ内容を先に登録していても（`is_reused` が `true` になるだけで）
    /// 安全に収束する。
    async fn persist_new(
        &self,
        new: NewMedia,
        actor_id: Option<i64>,
    ) -> Result<UploadOutcome, MediaStoreError> {
        let provider = select_provider(self.storage_providers, new.size).await?;
        check_quota(self.storage_providers, &provider, new.size).await?;

        let ext = match new.mime_type.as_str() {
            "image/jpeg" => "jpg",
            "image/png" => "png",
            "image/gif" => "gif",
            "image/avif" => "avif",
            _ => "webp",
        };
        let storage_key = format!("media/{}.{}", Uuid::new_v4(), ext);
        let s3 = S3StorageClient::new(&provider);
        s3.put(&storage_key, new.data, &new.mime_type)
            .await
            .map_err(|e| MediaStoreError::Storage(e.to_string()))?;

        let id = generate_snowflake_id(Utc::now());
        let (record, inserted) = self
            .media_files
            .upsert(CreateMediaFile {
                id,
                storage_provider_id: provider.id,
                sha256: new.sha256,
                blurhash: Some(new.blurhash),
                size: new.size,
                width: Some(new.width as i32),
                height: Some(new.height as i32),
                mime_type: new.mime_type,
                storage_key,
                duration_ms: None,
                thumbnail_key: None,
                uploaded_by_actor_id: actor_id,
                is_animated_image: new.is_animated_image,
            })
            .await
            .map_err(|e| MediaStoreError::Db(e.to_string()))?;
        Ok(UploadOutcome {
            record,
            is_reused: !inserted,
        })
    }

    /// `ImagePipeline` を重複排除チェックしつつ保存する。
    pub async fn store_image(
        &self,
        pipeline: ImagePipeline,
        actor_id: Option<i64>,
    ) -> Result<UploadOutcome, MediaStoreError> {
        match pipeline {
            ImagePipeline::AnimatedPassthrough(p) => {
                if let Some(existing) = self.find_existing(&p.sha256, &p.blurhash).await? {
                    return Ok(UploadOutcome {
                        record: existing,
                        is_reused: true,
                    });
                }
                let mut new_media: NewMedia = p.into();
                new_media.is_animated_image = true;
                self.persist_new(new_media, actor_id).await
            }
            ImagePipeline::Static { original, resized } => {
                if let Some(original) = &original {
                    if let Some(existing) = self
                        .find_existing(&original.sha256, &original.blurhash)
                        .await?
                    {
                        return Ok(UploadOutcome {
                            record: existing,
                            is_reused: true,
                        });
                    }
                }
                if let Some(existing) = self
                    .find_existing(&resized.sha256, &resized.blurhash)
                    .await?
                {
                    return Ok(UploadOutcome {
                        record: existing,
                        is_reused: true,
                    });
                }

                // どちらも未登録 → バイトサイズが小さい方（＝容量削減に実際に役立つ方）を採用する。
                // オリジナルの方が小さい/同サイズなら無劣化のオリジナルを優先する。
                let use_original = original
                    .as_ref()
                    .is_some_and(|o| o.data.len() <= resized.data.len());

                let new_media = if use_original {
                    original.unwrap().into()
                } else {
                    resized.into()
                };
                self.persist_new(new_media, actor_id).await
            }
        }
    }
}

//! 画像アップロードの重複排除・S3アップロード・DB登録を共通化するヘルパー。
//!
//! `storage::prepare_image()` が返す `ImagePipeline` には「Exif整理済み無劣化オリジナル」
//! と「Orientation適用・リサイズ・WebP化」の最大2候補が含まれる。ここでは各候補を
//! 順にDB重複チェックし、どちらも未登録ならバイトサイズが小さい方を採用して保存する
//! （ユーザーの画像を不要に劣化させないため）。
use chrono::Utc;
use uuid::Uuid;

use seiran_common::{
    generate_snowflake_id,
    repository::{CreateMediaFile, MediaFile},
    select_provider, ExifSanitizedImage, ImagePipeline, ProcessedImage, S3StorageClient,
};

use super::drive::{check_quota, map_selector_error};
use crate::{error::ApiError, AppState};

pub(crate) struct UploadOutcome {
    pub record: MediaFile,
    pub is_reused: bool,
}

struct NewMedia {
    sha256: String,
    blurhash: String,
    size: i64,
    width: u32,
    height: u32,
    mime_type: String,
    data: Vec<u8>,
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
        }
    }
}

async fn find_existing(state: &AppState, sha256: &str, blurhash: &str) -> Result<Option<MediaFile>, ApiError> {
    state
        .media_files
        .find_by_sha256_and_blurhash(sha256, blurhash)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))
}

async fn persist_new(state: &AppState, new: NewMedia, actor_id: Option<i64>) -> Result<MediaFile, ApiError> {
    let provider = select_provider(state.storage_providers.as_ref(), new.size)
        .await
        .map_err(map_selector_error)?;
    check_quota(state, &provider, new.size).await?;

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
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let id = generate_snowflake_id(Utc::now());
    state
        .media_files
        .insert(CreateMediaFile {
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
        })
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))
}

/// `ImagePipeline` を重複排除チェックしつつ保存する。
pub(crate) async fn store_image(
    state: &AppState,
    pipeline: ImagePipeline,
    actor_id: Option<i64>,
) -> Result<UploadOutcome, ApiError> {
    match pipeline {
        ImagePipeline::AnimatedPassthrough(p) => {
            if let Some(existing) = find_existing(state, &p.sha256, &p.blurhash).await? {
                return Ok(UploadOutcome { record: existing, is_reused: true });
            }
            let record = persist_new(state, p.into(), actor_id).await?;
            Ok(UploadOutcome { record, is_reused: false })
        }
        ImagePipeline::Static { original, resized } => {
            if let Some(original) = &original {
                if let Some(existing) = find_existing(state, &original.sha256, &original.blurhash).await? {
                    return Ok(UploadOutcome { record: existing, is_reused: true });
                }
            }
            if let Some(existing) = find_existing(state, &resized.sha256, &resized.blurhash).await? {
                return Ok(UploadOutcome { record: existing, is_reused: true });
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
            let record = persist_new(state, new_media, actor_id).await?;
            Ok(UploadOutcome { record, is_reused: false })
        }
    }
}

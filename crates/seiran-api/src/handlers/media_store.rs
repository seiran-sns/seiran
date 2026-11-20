//! 画像の保存（`seiran_common::storage::media_store`）を `AppState` から呼ぶための薄い
//! ラッパー。エラーを API のエラーへ変換する。
use seiran_common::{ImagePipeline, MediaStore, MediaStoreError};

pub(crate) use seiran_common::UploadOutcome;

use super::drive::map_selector_error;
use crate::{error::ApiError, AppState};

pub(crate) fn media_store(state: &AppState) -> MediaStore<'_> {
    MediaStore {
        media_files: state.media_files.as_ref(),
        storage_providers: state.storage_providers.as_ref(),
    }
}

pub(crate) fn map_media_store_error(e: MediaStoreError) -> ApiError {
    match e {
        MediaStoreError::Selector(e) => map_selector_error(e),
        MediaStoreError::QuotaExceeded => ApiError::InsufficientStorage,
        MediaStoreError::Storage(msg) | MediaStoreError::Db(msg) => ApiError::Internal(msg),
    }
}

/// `ImagePipeline` を重複排除チェックしつつ保存する。
pub(crate) async fn store_image(
    state: &AppState,
    pipeline: ImagePipeline,
    actor_id: Option<i64>,
) -> Result<UploadOutcome, ApiError> {
    media_store(state)
        .store_image(pipeline, actor_id)
        .await
        .map_err(map_media_store_error)
}

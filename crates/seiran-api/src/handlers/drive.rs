use axum::{
    extract::{Multipart, State},
    http::HeaderMap,
    Json,
};
use chrono::Utc;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use seiran_common::{
    ext_for_mime_type, generate_snowflake_id, is_allowed_video_or_audio_mime,
    process_image, probe_video_or_audio, sniff_mime_type, MediaKind,
    repository::CreateMediaFile,
    select_provider, SelectorError, S3StorageClient, StorageProviderRepository,
};

use crate::{
    error::ApiError,
    middleware::auth::extract_auth,
    AppState,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFileResponse {
    pub id: String,
    pub url: String,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blurhash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    pub size: i64,
    pub mime_type: String,
    pub is_reused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
}

/// POST /api/drive/files/create
///
/// multipart/form-data フィールド:
///   - `file`      : 画像バイナリ（必須）
///   - `media_type`: "avatar" | "banner" | "emoji" | "post"（省略時は "post"）
///
/// アクティブなストレージプロバイダーが設定されていない場合は 503 を返す。
/// ストレージプロバイダーの容量上限を超過する場合は 507 `STORAGE_QUOTA_EXCEEDED` を返す。
pub async fn create_drive_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<DriveFileResponse>, ApiError> {
    let auth = extract_auth(&headers, &state.local_auth).await?;

    // アップローダーの actor_id を取得
    let actor_id = state
        .actors
        .find_local_by_user_id(auth.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map(|a| a.id);

    // multipart フィールドを収集
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut media_type_str = "post".to_owned();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?
    {
        match field.name() {
            Some("file") => {
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::BadRequest(e.to_string()))?;
                file_bytes = Some(data.to_vec());
            }
            Some("media_type") => {
                media_type_str = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(e.to_string()))?;
            }
            _ => {}
        }
    }

    let raw_bytes = file_bytes.ok_or_else(|| ApiError::BadRequest("ファイルが含まれていません".to_owned()))?;

    let kind = match media_type_str.as_str() {
        "avatar" => MediaKind::Avatar,
        "banner" => MediaKind::Banner,
        "emoji"  => MediaKind::Emoji,
        _        => MediaKind::Post,
    };

    // マジックバイトで実際のファイル種別を判定する。
    let sniffed_mime = sniff_mime_type(&raw_bytes, "application/octet-stream");
    let is_image = sniffed_mime.starts_with("image/");

    // アバター・バナー・絵文字は画像限定（動画・音声は投稿添付のみ許可）
    if !matches!(kind, MediaKind::Post) && !is_image {
        return Err(ApiError::BadRequest("画像ファイルのみアップロードできます".to_owned()));
    }

    if is_image {
        return create_image_file(&state, actor_id, &raw_bytes, kind).await;
    }

    create_video_or_audio_file(&state, actor_id, raw_bytes, sniffed_mime).await
}

/// 画像アップロード処理（WebP 変換・リサイズ・blurhash 計算）。従来の処理をそのまま踏襲する。
async fn create_image_file(
    state: &AppState,
    actor_id: Option<i64>,
    raw_bytes: &[u8],
    kind: MediaKind,
) -> Result<Json<DriveFileResponse>, ApiError> {
    let processed = process_image(raw_bytes, kind)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // 重複排除: 同じ (sha256, blurhash) が既存ならそれを返す
    if let Some(existing) = state
        .media_files
        .find_by_sha256_and_blurhash(&processed.sha256, &processed.blurhash)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        let url = build_public_url(state.storage_providers.as_ref(), existing.storage_provider_id, &existing.storage_key).await;
        return Ok(Json(DriveFileResponse {
            id: existing.id.to_string(),
            url,
            sha256: existing.sha256,
            blurhash: existing.blurhash,
            width: existing.width.map(|w| w as u32),
            height: existing.height.map(|h| h as u32),
            size: existing.size,
            mime_type: existing.mime_type,
            is_reused: true,
            duration_ms: None,
            thumbnail_url: None,
        }));
    }

    let provider = select_provider(state.storage_providers.as_ref(), processed.size)
        .await
        .map_err(map_selector_error)?;
    check_quota(state, &provider, processed.size).await?;

    // S3 アップロード（拡張子は実際の MIME type に合わせる）
    let ext = match processed.mime_type.as_str() {
        "image/jpeg" => "jpg",
        "image/png"  => "png",
        "image/gif"  => "gif",
        "image/avif" => "avif",
        _            => "webp",
    };
    let storage_key = format!("media/{}.{}", Uuid::new_v4(), ext);
    let s3 = S3StorageClient::new(&provider);
    let public_url = s3
        .put(&storage_key, processed.data, &processed.mime_type)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let id = generate_snowflake_id(Utc::now());
    let record = state
        .media_files
        .insert(CreateMediaFile {
            id,
            storage_provider_id: provider.id,
            sha256: processed.sha256.clone(),
            blurhash: Some(processed.blurhash.clone()),
            size: processed.size,
            width: Some(processed.width as i32),
            height: Some(processed.height as i32),
            mime_type: processed.mime_type.clone(),
            storage_key,
            duration_ms: None,
            thumbnail_key: None,
            uploaded_by_actor_id: actor_id,
        })
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(DriveFileResponse {
        id: record.id.to_string(),
        url: public_url,
        sha256: processed.sha256,
        blurhash: Some(processed.blurhash),
        width: Some(processed.width),
        height: Some(processed.height),
        size: processed.size,
        mime_type: processed.mime_type,
        is_reused: false,
        duration_ms: None,
        thumbnail_url: None,
    }))
}

/// 動画・音声アップロード処理。トランスコードはせず原本をそのまま保存し、
/// ffmpeg でメタデータ（再生時間・解像度）とサムネイルフレームのみ抽出する。
async fn create_video_or_audio_file(
    state: &AppState,
    actor_id: Option<i64>,
    raw_bytes: Vec<u8>,
    mime_type: String,
) -> Result<Json<DriveFileResponse>, ApiError> {
    if !is_allowed_video_or_audio_mime(&mime_type) {
        return Err(ApiError::BadRequest(format!("対応していないファイル形式です: {}", mime_type)));
    }

    let sha256 = hex::encode(Sha256::digest(&raw_bytes));
    let size = raw_bytes.len() as i64;

    let probed = probe_video_or_audio(&raw_bytes, ext_for_mime_type(&mime_type)).await;
    let thumbnail = probed.thumbnail_frame
        .as_deref()
        .and_then(|frame| process_image(frame, MediaKind::Post).ok());
    let blurhash = thumbnail.as_ref().map(|t| t.blurhash.clone());

    // 重複排除: blurhash が求まった（動画）場合は (sha256, blurhash) 一致、
    // そうでない（音声）場合は sha256 のみ（blurhash IS NULL の行に限定）で判定する。
    let existing = if let Some(bh) = &blurhash {
        state.media_files.find_by_sha256_and_blurhash(&sha256, bh).await
    } else {
        state.media_files.find_by_sha256(&sha256).await
    }
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    if let Some(existing) = existing {
        let url = build_public_url(state.storage_providers.as_ref(), existing.storage_provider_id, &existing.storage_key).await;
        let thumbnail_url = match &existing.thumbnail_key {
            Some(key) => Some(build_public_url(state.storage_providers.as_ref(), existing.storage_provider_id, key).await),
            None => None,
        };
        return Ok(Json(DriveFileResponse {
            id: existing.id.to_string(),
            url,
            sha256: existing.sha256,
            blurhash: existing.blurhash,
            width: existing.width.map(|w| w as u32),
            height: existing.height.map(|h| h as u32),
            size: existing.size,
            mime_type: existing.mime_type,
            is_reused: true,
            duration_ms: existing.duration_ms.map(|d| d as i64),
            thumbnail_url,
        }));
    }

    let provider = select_provider(state.storage_providers.as_ref(), size)
        .await
        .map_err(map_selector_error)?;
    check_quota(state, &provider, size).await?;

    let s3 = S3StorageClient::new(&provider);

    // サムネイルを先にアップロード（本体より小さく、失敗時のロールバック対象が少なくて済む）
    let thumbnail_key = if let Some(ref thumb) = thumbnail {
        let key = format!("media/{}-thumb.webp", Uuid::new_v4());
        s3.put(&key, thumb.data.clone(), "image/webp")
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        Some(key)
    } else {
        None
    };

    let ext = ext_for_mime_type(&mime_type);
    let storage_key = format!("media/{}.{}", Uuid::new_v4(), ext);
    let public_url = s3
        .put(&storage_key, raw_bytes, &mime_type)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let id = generate_snowflake_id(Utc::now());
    let record = state
        .media_files
        .insert(CreateMediaFile {
            id,
            storage_provider_id: provider.id,
            sha256: sha256.clone(),
            blurhash: blurhash.clone(),
            size,
            width: probed.width.map(|w| w as i32),
            height: probed.height.map(|h| h as i32),
            mime_type: mime_type.clone(),
            storage_key,
            duration_ms: probed.duration_ms.map(|d| d as i32),
            thumbnail_key: thumbnail_key.clone(),
            uploaded_by_actor_id: actor_id,
        })
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let thumbnail_url = match &thumbnail_key {
        Some(key) => Some(build_public_url(state.storage_providers.as_ref(), provider.id, key).await),
        None => None,
    };

    Ok(Json(DriveFileResponse {
        id: record.id.to_string(),
        url: public_url,
        sha256,
        blurhash,
        width: probed.width,
        height: probed.height,
        size,
        mime_type,
        is_reused: false,
        duration_ms: probed.duration_ms,
        thumbnail_url,
    }))
}

fn map_selector_error(e: SelectorError) -> ApiError {
    match e {
        SelectorError::NoAvailableProvider => ApiError::ServiceUnavailable("ストレージプロバイダーが設定されていません"),
        SelectorError::QuotaExceeded => ApiError::InsufficientStorage,
        SelectorError::Db(db_e) => ApiError::Internal(db_e.to_string()),
    }
}

/// クォータ二重チェック（プロバイダー確定後、PUT 前に明示的に確認）
async fn check_quota(state: &AppState, provider: &seiran_common::repository::StorageProvider, upload_size: i64) -> Result<(), ApiError> {
    if let Some(cap_mb) = provider.capacity_mb {
        let used_bytes = state
            .storage_providers
            .get_used_bytes(provider.id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        if cap_mb * 1024 * 1024 < used_bytes + upload_size {
            return Err(ApiError::InsufficientStorage);
        }
    }
    Ok(())
}

/// 既存レコードの公開 URL を provider の public_url + storage_key で組み立てる。
/// provider が取得できなかった場合は storage_key をそのまま返す（フォールバック）。
async fn build_public_url(
    providers: &dyn StorageProviderRepository,
    provider_id: i64,
    storage_key: &str,
) -> String {
    match providers.find_by_id(provider_id).await {
        Ok(Some(p)) => format!("{}/{}", p.public_url.trim_end_matches('/'), storage_key),
        _ => storage_key.to_owned(),
    }
}

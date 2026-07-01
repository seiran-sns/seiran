use axum::{
    extract::{Multipart, State},
    http::HeaderMap,
    Json,
};
use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use seiran_common::{
    generate_snowflake_id,
    process_image, MediaKind,
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
    pub blurhash: String,
    pub width: u32,
    pub height: u32,
    pub size: i64,
    pub mime_type: String,
    pub is_reused: bool,
}

/// POST /api/drive/files/create
///
/// multipart/form-data フィールド:
///   - `file`      : 画像バイナリ（必須）
///   - `media_type`: "avatar" | "banner" | "emoji" | "post"（省略時は "post"）
///
/// アクティブなストレージプロバイダーが設定されていない場合は 503 を返す。
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

    // 画像処理（WebP 変換・リサイズ）
    let processed = process_image(&raw_bytes, kind)
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
            width: existing.width as u32,
            height: existing.height as u32,
            size: existing.size,
            mime_type: existing.mime_type,
            is_reused: true,
        }));
    }

    // ストレージ選択
    let provider = select_provider(state.storage_providers.as_ref(), processed.size)
        .await
        .map_err(|e| match e {
            SelectorError::NoAvailableProvider => ApiError::ServiceUnavailable("ストレージプロバイダーが設定されていないか、全て容量超過です"),
            SelectorError::Db(db_e) => ApiError::Internal(db_e.to_string()),
        })?;

    // S3 アップロード
    let storage_key = format!("media/{}.webp", Uuid::new_v4());
    let s3 = S3StorageClient::new(&provider);
    let public_url = s3
        .put(&storage_key, processed.data, &processed.mime_type)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // DB に記録
    let id = generate_snowflake_id(Utc::now());
    let record = state
        .media_files
        .insert(CreateMediaFile {
            id,
            storage_provider_id: provider.id,
            sha256: processed.sha256.clone(),
            blurhash: processed.blurhash.clone(),
            size: processed.size,
            width: processed.width as i32,
            height: processed.height as i32,
            mime_type: processed.mime_type.clone(),
            storage_key,
            uploaded_by_actor_id: actor_id,
        })
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(DriveFileResponse {
        id: record.id.to_string(),
        url: public_url,
        sha256: processed.sha256,
        blurhash: processed.blurhash,
        width: processed.width,
        height: processed.height,
        size: processed.size,
        mime_type: processed.mime_type,
        is_reused: false,
    }))
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

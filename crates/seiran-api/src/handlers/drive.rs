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
    atp::sign_service_auth_jwt,
    ext_for_mime_type, generate_snowflake_id, is_allowed_video_or_audio_mime,
    process_image, probe_video_or_audio, sniff_mime_type, MediaKind,
    queue::worker::priority,
    repository::{Actor, CreateMediaFile},
    select_provider, Job, SelectorError, S3StorageClient, StorageProviderRepository,
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

    // アップローダーのアクター情報を取得（Bsky動画パイプライン結合にはDID/署名鍵も必要）
    let actor = state
        .actors
        .find_local_by_user_id(auth.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let actor_id = actor.as_ref().map(|a| a.id);

    // multipart フィールドを収集
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut media_type_str = "post".to_owned();
    let mut deliver_to_bsky = true;

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
            Some("deliver_to_bsky") => {
                let v = field.text().await.map_err(|e| ApiError::BadRequest(e.to_string()))?;
                deliver_to_bsky = v != "false";
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

    create_video_or_audio_file(&state, actor.as_ref(), raw_bytes, sniffed_mime, deliver_to_bsky).await
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
    actor: Option<&Actor>,
    raw_bytes: Vec<u8>,
    mime_type: String,
    deliver_to_bsky: bool,
) -> Result<Json<DriveFileResponse>, ApiError> {
    let actor_id = actor.map(|a| a.id);
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

    // Bsky動画パイプラインへの提出にはS3保存後も生バイト列が要るため、
    // move される前に複製しておく（音声やdeliver_to_bsky=falseでは複製しない）。
    let video_bytes_for_bsky = if mime_type.starts_with("video/") && deliver_to_bsky {
        Some(raw_bytes.clone())
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

    // 動画かつBsky配信あり要求時、Bluesky公式動画パイプラインへ提出する。
    // uploadVideo自体は同期・即時（ジョブ受理確認のみ）で、アップロードAPIの
    // レスポンスはブロックしない。完了待ちは`Job::BskyVideoPoll`に委ねる。
    if let (Some(video_bytes), Some(actor)) = (video_bytes_for_bsky, actor) {
        submit_to_bsky_video_pipeline(state, actor, record.id, video_bytes).await;
    }

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

const BSKY_VIDEO_SERVICE_HOST: &str = "https://video.bsky.app";

/// Bluesky公式動画パイプライン（`app.bsky.video.uploadVideo`）へ動画を提出する。
/// 同期的だが即時（ジョブ受理確認のみ）で、完了までは待たない
/// （`Job::BskyVideoPoll`がバックグラウンドで完了を待つ）。
/// 失敗しても`media_files.bsky_video_status='failed'`を記録するだけで、
/// アップロードAPI自体は成功として扱う（動画はローカル保存済みのため）。
async fn submit_to_bsky_video_pipeline(state: &AppState, actor: &Actor, media_file_id: i64, video_bytes: Vec<u8>) {
    let (Some(did), Some(pem)) = (actor.at_did.as_deref(), actor.at_signing_key_pem.as_deref()) else {
        tracing::warn!("[BskyVideo] at_did/at_signing_key_pem 未設定のためスキップ media_file_id={}", media_file_id);
        mark_bsky_video_failed(state, media_file_id).await;
        return;
    };

    let own_pds_did = format!("did:web:{}", state.local_domain);
    let jwt = match sign_service_auth_jwt(pem, did, &own_pds_did, "com.atproto.repo.uploadBlob") {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("[BskyVideo] JWT署名失敗 media_file_id={}: {}", media_file_id, e);
            mark_bsky_video_failed(state, media_file_id).await;
            return;
        }
    };

    let upload_url = format!(
        "{}/xrpc/app.bsky.video.uploadVideo?did={}&name=seiran-{}.mp4",
        BSKY_VIDEO_SERVICE_HOST,
        urlencoding::encode(did),
        media_file_id,
    );

    let resp = state
        .ap_client
        .http
        .post(&upload_url)
        .header("Authorization", format!("Bearer {}", jwt))
        .header("Content-Type", "video/mp4")
        .body(video_bytes)
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[BskyVideo] uploadVideoリクエスト失敗 media_file_id={}: {}", media_file_id, e);
            mark_bsky_video_failed(state, media_file_id).await;
            return;
        }
    };

    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();

    // 同一内容の動画が既にBluesky側で処理済みの場合、409 Conflict + "already_exists"
    // だが有効なjobIdは返ってくる（実機確認済み）。この場合も成功として扱う。
    let job_id = serde_json::from_str::<serde_json::Value>(&body_text)
        .ok()
        .and_then(|v| {
            v.get("jobId").and_then(|j| j.as_str()).map(|s| s.to_string())
                .or_else(|| v.get("jobStatus").and_then(|j| j.get("jobId")).and_then(|j| j.as_str()).map(|s| s.to_string()))
        });

    let Some(job_id) = job_id else {
        tracing::error!("[BskyVideo] uploadVideo失敗 media_file_id={} status={} body={}", media_file_id, status, body_text);
        mark_bsky_video_failed(state, media_file_id).await;
        return;
    };
    if !status.is_success() {
        tracing::info!("[BskyVideo] uploadVideo非2xxだがjobId取得 media_file_id={} status={} jobId={}", media_file_id, status, job_id);
    }

    if let Err(e) = sqlx::query(
        "UPDATE media_files SET bsky_video_job_id = $1, bsky_video_status = 'pending' WHERE id = $2",
    )
    .bind(&job_id)
    .bind(media_file_id)
    .execute(&state.db)
    .await
    {
        tracing::error!("[BskyVideo] DB更新失敗 media_file_id={}: {}", media_file_id, e);
        return;
    }

    if let Err(e) = state.job_queue.enqueue(Job::BskyVideoPoll { media_file_id }, priority::HIGH).await {
        tracing::error!("[BskyVideo] ジョブ投入失敗 media_file_id={}: {}", media_file_id, e);
    }
}

async fn mark_bsky_video_failed(state: &AppState, media_file_id: i64) {
    let _ = sqlx::query("UPDATE media_files SET bsky_video_status = 'failed' WHERE id = $1")
        .bind(media_file_id)
        .execute(&state.db)
        .await;
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

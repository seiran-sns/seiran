use axum::{
    extract::{Multipart, Path, State},
    http::HeaderMap,
    response::{Html, IntoResponse},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use seiran_common::{
    atp::sign_service_auth_jwt,
    convert_audio_to_gray_video, ext_for_mime_type, generate_snowflake_id, is_allowed_video_or_audio_mime,
    process_image, probe_video_or_audio, sniff_mime_type, MediaKind,
    queue::worker::priority,
    repository::{Actor, CreateMediaFile},
    select_provider, Job, SelectorError, S3StorageClient, StorageProviderRepository,
};

use crate::{
    error::ApiError,
    middleware::auth::{extract_auth, AuthUser},
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
    // 以下、Misskeyワイヤー互換用（misskey_dart `DriveFile.fromJson` の必須フィールド）。
    // seiranのフロントエンドは使わないが、Misskeyクライアント（Aria等）のJSONデコードが
    // これらの欠落でnullキャスト例外を起こすため必須。
    pub created_at: DateTime<Utc>,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub md5: String,
    pub is_sensitive: bool,
    pub properties: DriveFileProperties,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFileProperties {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
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
    // multipart フィールドを収集
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut filename_from_file_field: Option<String> = None;
    let mut filename_from_name_field: Option<String> = None;
    let mut media_type_str = "post".to_owned();
    let mut deliver_to_bsky = true;
    let mut token_field: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?
    {
        match field.name() {
            Some("file") => {
                filename_from_file_field = field.file_name().map(|s| s.to_owned());
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
            // Misskeyクライアント（Aria等）はアクセストークンをAuthorizationヘッダーではなく
            // multipartの`i`フィールドとして送ってくる（misskey_dart postWithBinary仕様）。
            // JSON/クエリ用のmisskey_auth_bridgeはmultipartボディを素通りするため、ここで拾う。
            Some("i") => {
                let v = field.text().await.map_err(|e| ApiError::BadRequest(e.to_string()))?;
                if !v.is_empty() {
                    token_field = Some(v);
                }
            }
            // Misskey本家の`DriveFilesCreateRequest.name`。misskey_dartの`postWithBinary`は
            // file添付にContent-Dispositionのfilenameを付与せず、代わりにこの独立した
            // テキストフィールドでファイル名を送る（`createAsBinary`がfileName引数を渡さない
            // ため）。file添付側のfilename属性より優先する。
            Some("name") => {
                let v = field.text().await.map_err(|e| ApiError::BadRequest(e.to_string()))?;
                if !v.is_empty() {
                    filename_from_name_field = Some(v);
                }
            }
            _ => {}
        }
    }

    let original_filename = filename_from_name_field.or(filename_from_file_field);

    let auth = match extract_auth(&headers, &state.local_auth).await {
        Ok(auth) => auth,
        Err(err) => {
            let token = token_field.ok_or(err)?;
            let verified = state
                .local_auth
                .verify_token(&token)
                .map_err(|_| ApiError::Unauthorized("トークンが無効です"))?;
            AuthUser { user_id: verified.user_id, email: verified.email }
        }
    };

    // アップローダーのアクター情報を取得（Bsky動画パイプライン結合にはDID/署名鍵も必要）
    let actor = state
        .actors
        .find_local_by_user_id(auth.user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let actor_id = actor.as_ref().map(|a| a.id);

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

    let md5 = format!("{:x}", md5::compute(&raw_bytes));

    if is_image {
        return create_image_file(&state, actor_id, &raw_bytes, kind, md5, original_filename).await;
    }

    create_video_or_audio_file(&state, actor.as_ref(), raw_bytes, sniffed_mime, deliver_to_bsky, md5, original_filename).await
}

/// 画像アップロード処理（WebP 変換・リサイズ・blurhash 計算）。従来の処理をそのまま踏襲する。
async fn create_image_file(
    state: &AppState,
    actor_id: Option<i64>,
    raw_bytes: &[u8],
    kind: MediaKind,
    md5: String,
    original_filename: Option<String>,
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
        // 画像は別途縮小サムネイルを持たないため、本体と同じURLをthumbnailUrlとして返す。
        // Misskeyクライアント（Aria等）はDriveFile.thumbnailUrlが無いと投稿フォームの
        // プレビューで画像を表示せずアイコンにフォールバックするため必須。
        let thumbnail_url = Some(url.clone());
        let name = original_filename.unwrap_or_else(|| default_file_name(existing.id, &existing.mime_type));
        let kind = existing.mime_type.clone();
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
            thumbnail_url,
            created_at: existing.created_at,
            name,
            kind,
            md5,
            is_sensitive: false,
            properties: DriveFileProperties {
                width: existing.width.map(|w| w as u32),
                height: existing.height.map(|h| h as u32),
            },
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

    let name = original_filename.unwrap_or_else(|| default_file_name(record.id, &processed.mime_type));
    let kind = processed.mime_type.clone();
    // 画像は別途縮小サムネイルを持たないため、本体と同じURLをthumbnailUrlとして返す
    // （Misskeyクライアントの投稿フォームプレビュー用、既存レコード再利用時と同じ理由）。
    let thumbnail_url = Some(public_url.clone());
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
        thumbnail_url,
        created_at: record.created_at,
        name,
        kind,
        md5,
        is_sensitive: false,
        properties: DriveFileProperties {
            width: Some(processed.width),
            height: Some(processed.height),
        },
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
    md5: String,
    original_filename: Option<String>,
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
        // 同一 sha256 の既存レコードを再利用する場合でも、過去の Bsky 動画パイプライン
        // 提出が 'failed'（またはそもそも未提出）のままだと、再アップロードしても
        // 永久に video embed 化されない（isReused の早期 return で submit が
        // スキップされていた既存バグ）。再送信を試みる。音声は Bsky には専用embedが
        // 無いため、グレー背景動画に変換してから提出する（2026-07-17 マイケル発案）。
        let existing_is_video = existing.mime_type.starts_with("video/");
        let existing_is_audio = existing.mime_type.starts_with("audio/");
        if (existing_is_video || existing_is_audio) && deliver_to_bsky {
            if let Some(actor) = actor {
                let status: Option<String> = sqlx::query_scalar(
                    "SELECT bsky_video_status FROM media_files WHERE id = $1",
                )
                .bind(existing.id)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();
                if !matches!(status.as_deref(), Some("pending") | Some("ready")) {
                    let bytes_for_pipeline = if existing_is_audio {
                        convert_audio_to_gray_video(&raw_bytes, ext_for_mime_type(&existing.mime_type)).await
                    } else {
                        Some(raw_bytes.clone())
                    };
                    if let Some(bytes_for_pipeline) = bytes_for_pipeline {
                        submit_to_bsky_video_pipeline(state, actor, existing.id, bytes_for_pipeline).await;
                    }
                }
            }
        }

        let url = build_public_url(state.storage_providers.as_ref(), existing.storage_provider_id, &existing.storage_key).await;
        let thumbnail_url = match &existing.thumbnail_key {
            Some(key) => Some(build_public_url(state.storage_providers.as_ref(), existing.storage_provider_id, key).await),
            None => None,
        };
        let name = original_filename.unwrap_or_else(|| default_file_name(existing.id, &existing.mime_type));
        let kind = existing.mime_type.clone();
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
            created_at: existing.created_at,
            name,
            kind,
            md5,
            is_sensitive: false,
            properties: DriveFileProperties {
                width: existing.width.map(|w| w as u32),
                height: existing.height.map(|h| h as u32),
            },
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

    // Bsky動画パイプラインへの提出用バイト列を用意する（deliver_to_bsky=falseでは不要）。
    // 動画はそのまま、音声は「グレー背景515x75の静止画 + 音声トラック」のmp4に変換して
    // 提出する。Bskyには音声専用embedが無く動画embedしか無いため
    // （2026-07-17 マイケル発案）。
    let video_bytes_for_bsky = if !deliver_to_bsky {
        None
    } else if mime_type.starts_with("video/") {
        Some(raw_bytes.clone())
    } else if mime_type.starts_with("audio/") {
        convert_audio_to_gray_video(&raw_bytes, ext_for_mime_type(&mime_type)).await
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

    let name = original_filename.unwrap_or_else(|| default_file_name(record.id, &mime_type));
    let kind = mime_type.clone();
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
        created_at: record.created_at,
        name,
        kind,
        md5,
        is_sensitive: false,
        properties: DriveFileProperties {
            width: probed.width,
            height: probed.height,
        },
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
    // jobIdが空文字列の場合は「失敗レスポンスにたまたまjobIdフィールドが存在した」
    // だけなので無効扱いする（2026-07-17、これが原因でエラー詳細が握り潰されていた
    // バグを修正）。
    let job_id = serde_json::from_str::<serde_json::Value>(&body_text)
        .ok()
        .and_then(|v| {
            v.get("jobId").and_then(|j| j.as_str()).map(|s| s.to_string())
                .or_else(|| v.get("jobStatus").and_then(|j| j.get("jobId")).and_then(|j| j.as_str()).map(|s| s.to_string()))
        })
        .filter(|s| !s.is_empty());

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

/// Misskeyワイヤー互換の`name`フィールド用。クライアントが元のファイル名を送ってこなかった
/// 場合のフォールバック名を、MIME typeから推測した拡張子付きで生成する。
fn default_file_name(id: i64, mime_type: &str) -> String {
    let ext = match mime_type {
        "image/jpeg" => "jpg",
        "image/png"  => "png",
        "image/gif"  => "gif",
        "image/avif" => "avif",
        "image/webp" => "webp",
        _            => ext_for_mime_type(mime_type),
    };
    format!("seiran-{}.{}", id, ext)
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

/// `GET /api/media/:media_file_id/watch`
///
/// 音声・動画の簡易視聴ページ。Bsky の `app.bsky.embed.external` は音声専用の
/// embed typeを持たず、動画も `bsky_video_status='ready'` でなければ external
/// フォールバックになる（`AtpCommitService::commit_post`）。そのリンク先が
/// メディアファイルの直リンクだとブラウザがダウンロードしてしまい再生できない
/// ため、`<audio>`/`<video>` タグ1個だけの簡素なHTMLを返す
/// （2026-07-17 マイケル指摘）。
pub async fn watch_media(
    Path(media_file_id): Path<i64>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let mf = match state.media_files.find_by_id(media_file_id).await {
        Ok(Some(mf)) => mf,
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => {
            tracing::error!("[watch_media] DB エラー: {}", e);
            return ApiError::Internal(e.to_string()).into_response();
        }
    };

    let url = build_public_url(state.storage_providers.as_ref(), mf.storage_provider_id, &mf.storage_key).await;
    let tag = if mf.mime_type.starts_with("video/") { "video" } else { "audio" };
    let html = format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>seiran メディア</title>
<style>
html,body{{margin:0;height:100%;background:#000;display:flex;align-items:center;justify-content:center}}
{tag}{{max-width:100%;max-height:100%}}
</style>
</head>
<body>
<{tag} src="{url}" controls autoplay playsinline></{tag}>
</body>
</html>"#
    );
    Html(html).into_response()
}

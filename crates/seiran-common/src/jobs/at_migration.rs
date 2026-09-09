//! 既存DID転入フロー（`docs/account_migration.md`）の単発ジョブ群。
//!
//! `follow_import`と異なり自己再enqueue型ではなく、各ステップ1回限りの単発ジョブ。
//! `request_id`単位のadvisory lockは起動時リカバリでの重複投入対策として全ジョブ共通で使う
//! （`jobs::follow_import`のコメント参照、同じ理由）。
//!
//! 失敗時の扱い: 一時的エラー（ネットワーク断等）は`JobError::Transient`を返し
//! `at_migration_requests.status`は変更しない（WorkerEngineが指数バックオフで自動リトライ、
//! ユーザーもUIの「リトライ」ボタンでいつでも同じジョブを再投入できる）。CARデコード失敗等、
//! リトライしても結果が変わらない恒久的エラーは`JobError::Permanent`を返す前に
//! `status='failed'`へ明示的に遷移させ、UIに「失敗」を表示できるようにする。

use std::sync::Arc;

use sqlx::PgPool;

use crate::atp::did_resolve::resolve_stored_endpoint;
use crate::atp::migration_client::{
    fetch_repo_car, list_blobs, request_plc_operation_signature, AtpSession,
};
use crate::atp::mst_walk::decode_repo_records;
use crate::queue::worker::JobContext;
use crate::repository::{
    AtMigrationRepository, PgAtMigrationRepository, PgSiteSettingsRepository,
    SiteSettingsRepository, StagedRecord,
};
use crate::traits::JobError;

/// `Job::MigrationFetchRepo` — PDS Aから`getRepo`(CAR)+`listBlobs`を取得し、
/// `at_migration_records`/`at_migration_blobs`へステージングする。
pub async fn handle_fetch_repo(request_id: i64, ctx: Arc<JobContext>) -> Result<(), JobError> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        return Err(JobError::Permanent(
            "[MigrationFetchRepo] DB pool 未設定".to_string(),
        ));
    };

    let Some(lock_conn) = crate::advisory_lock::try_acquire(pool, request_id)
        .await
        .map_err(JobError::Transient)?
    else {
        tracing::info!(
            "[MigrationFetchRepo] request_id={} は既に別のジョブが処理中のためスキップ",
            request_id
        );
        return Ok(());
    };

    let result = process_fetch_repo(request_id, pool, &ctx).await;

    crate::advisory_lock::release(lock_conn, request_id).await;

    result
}

async fn process_fetch_repo(
    request_id: i64,
    pool: &PgPool,
    ctx: &JobContext,
) -> Result<(), JobError> {
    let repo = PgAtMigrationRepository::new(pool.clone());

    let Some(req) = repo
        .get(request_id)
        .await
        .map_err(|e| JobError::Transient(format!("[MigrationFetchRepo] リクエスト取得失敗: {e}")))?
    else {
        tracing::warn!(
            "[MigrationFetchRepo] request_id={} が見つかりません（終了）",
            request_id
        );
        return Ok(());
    };

    if req.status != "fetching_repo" {
        tracing::info!(
            "[MigrationFetchRepo] request_id={} は status={} のため終了",
            request_id,
            req.status
        );
        return Ok(());
    }

    let now = chrono::Utc::now();
    let session = AtpSession {
        did: req.source_did.clone(),
        handle: req.source_handle.clone(),
        access_jwt: req.source_access_jwt.clone().unwrap_or_default(),
        refresh_jwt: req.source_refresh_jwt.clone().unwrap_or_default(),
    };

    // `start`時点で確定したPDS Aのエンドポイント文字列をそのまま使う（DID文書から
    // 再導出しない——`resolve_stored_endpoint`のドキュメントコメント参照）。
    // SSRF対策としてのIP検証だけは実行の都度やり直す（DNS rebinding対策）。
    let resolved = resolve_stored_endpoint(&req.source_pds_endpoint)
        .await
        .map_err(|e| {
            JobError::Transient(format!("[MigrationFetchRepo] PDSエンドポイント検証失敗: {e}"))
        })?;

    let car = fetch_repo_car(&resolved, &session).await.map_err(|e| {
        JobError::Transient(format!("[MigrationFetchRepo] getRepo失敗: {e}"))
    })?;

    let records = match decode_repo_records(&car) {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("[MigrationFetchRepo] CARデコード失敗（不正なリポジトリデータ）: {e}");
            repo.set_failed(request_id, "failed", &msg, now)
                .await
                .map_err(|e| JobError::Transient(format!("[MigrationFetchRepo] 失敗記録失敗: {e}")))?;
            return Err(JobError::Permanent(msg));
        }
    };

    let staged: Vec<StagedRecord> = records
        .into_iter()
        .map(|r| StagedRecord {
            collection: r.collection,
            rkey: r.rkey,
            cid: r.cid.to_string(),
            bytes: r.bytes,
        })
        .collect();
    let staged_count = staged.len();
    repo.stage_records(request_id, &staged).await.map_err(|e| {
        JobError::Transient(format!("[MigrationFetchRepo] レコードステージング失敗: {e}"))
    })?;

    let blob_cids = list_blobs(&resolved, &session).await.map_err(|e| {
        JobError::Transient(format!("[MigrationFetchRepo] listBlobs失敗: {e}"))
    })?;
    let blob_count = blob_cids.len();
    repo.stage_blobs(request_id, &blob_cids).await.map_err(|e| {
        JobError::Transient(format!("[MigrationFetchRepo] blobステージング失敗: {e}"))
    })?;

    let site_settings = PgSiteSettingsRepository::new(pool.clone());
    let require_ev = site_settings
        .get("require_email_verification")
        .await
        .map_err(|e| {
            JobError::Transient(format!("[MigrationFetchRepo] site_settings取得失敗: {e}"))
        })?
        .map(|v| v == "true")
        .unwrap_or(false);
    let next_status = if require_ev {
        "awaiting_seiran_email"
    } else {
        "requesting_plc_signature"
    };

    repo.set_status(request_id, next_status, now)
        .await
        .map_err(|e| JobError::Transient(format!("[MigrationFetchRepo] ステータス更新失敗: {e}")))?;

    tracing::info!(
        "[MigrationFetchRepo] request_id={} 完了 (records={}, blobs={}, next={})",
        request_id,
        staged_count,
        blob_count,
        next_status
    );

    // require_email_verification=falseならメール確認を挟まず即座に次段へ進める
    // （ONの場合は`confirm-seiran-email`エンドポイントがユーザーの確認後に積む）。
    if next_status == "requesting_plc_signature" {
        if let Err(e) = ctx
            .queue
            .enqueue(
                crate::traits::Job::MigrationRequestPlcSignature { request_id },
                crate::queue::worker::priority::NORMAL,
            )
            .await
        {
            tracing::error!(
                "[MigrationFetchRepo] MigrationRequestPlcSignature enqueue失敗 (request_id={}): {}",
                request_id,
                e
            );
        }
    }

    Ok(())
}

/// `Job::MigrationRequestPlcSignature` — PDS Aへ`requestPlcOperationSignature`を呼び、
/// PDS A登録メール宛に確認コードを送付させる。
pub async fn handle_request_plc_signature(
    request_id: i64,
    ctx: Arc<JobContext>,
) -> Result<(), JobError> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        return Err(JobError::Permanent(
            "[MigrationRequestPlcSignature] DB pool 未設定".to_string(),
        ));
    };

    let Some(lock_conn) = crate::advisory_lock::try_acquire(pool, request_id)
        .await
        .map_err(JobError::Transient)?
    else {
        tracing::info!(
            "[MigrationRequestPlcSignature] request_id={} は既に別のジョブが処理中のためスキップ",
            request_id
        );
        return Ok(());
    };

    let result = process_request_plc_signature(request_id, pool).await;

    crate::advisory_lock::release(lock_conn, request_id).await;

    result
}

async fn process_request_plc_signature(request_id: i64, pool: &PgPool) -> Result<(), JobError> {
    let repo = PgAtMigrationRepository::new(pool.clone());

    let Some(req) = repo.get(request_id).await.map_err(|e| {
        JobError::Transient(format!("[MigrationRequestPlcSignature] リクエスト取得失敗: {e}"))
    })?
    else {
        tracing::warn!(
            "[MigrationRequestPlcSignature] request_id={} が見つかりません（終了）",
            request_id
        );
        return Ok(());
    };

    if req.status != "requesting_plc_signature" {
        tracing::info!(
            "[MigrationRequestPlcSignature] request_id={} は status={} のため終了",
            request_id,
            req.status
        );
        return Ok(());
    }

    let session = AtpSession {
        did: req.source_did.clone(),
        handle: req.source_handle.clone(),
        access_jwt: req.source_access_jwt.clone().unwrap_or_default(),
        refresh_jwt: req.source_refresh_jwt.clone().unwrap_or_default(),
    };

    let resolved = resolve_stored_endpoint(&req.source_pds_endpoint)
        .await
        .map_err(|e| {
            JobError::Transient(format!(
                "[MigrationRequestPlcSignature] PDSエンドポイント検証失敗: {e}"
            ))
        })?;

    request_plc_operation_signature(&resolved, &session)
        .await
        .map_err(|e| {
            JobError::Transient(format!(
                "[MigrationRequestPlcSignature] requestPlcOperationSignature失敗: {e}"
            ))
        })?;

    repo.set_status(request_id, "awaiting_plc_token", chrono::Utc::now())
        .await
        .map_err(|e| {
            JobError::Transient(format!("[MigrationRequestPlcSignature] ステータス更新失敗: {e}"))
        })?;

    tracing::info!(
        "[MigrationRequestPlcSignature] request_id={} 完了（PDS A登録メールへ確認コード送付済み）",
        request_id
    );

    Ok(())
}

/// `process_import`が返す「unlock後にやるべきこと」（`jobs::follow_import`と同じ設計）。
enum ImportNextAction {
    Continue,
    Stop,
}

/// `Job::MigrationImportProcess` — 自己再enqueue型。`at_migration_records`→
/// `at_migration_blobs`の順に未取り込み分を1件ずつ実体化し、尽きたら
/// `deactivating_source`へ進める。`follow_import`と同じ`request_id`単位advisory lock。
pub async fn handle_import_process(request_id: i64, ctx: Arc<JobContext>) -> Result<(), JobError> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        return Err(JobError::Permanent(
            "[MigrationImportProcess] DB pool 未設定".to_string(),
        ));
    };

    let Some(lock_conn) = crate::advisory_lock::try_acquire(pool, request_id)
        .await
        .map_err(JobError::Transient)?
    else {
        tracing::info!(
            "[MigrationImportProcess] request_id={} は既に別のジョブが処理中のためスキップ",
            request_id
        );
        return Ok(());
    };

    let result = process_import(request_id, pool, &ctx).await;

    crate::advisory_lock::release(lock_conn, request_id).await;

    match result? {
        ImportNextAction::Continue => {
            if let Err(e) = ctx
                .queue
                .enqueue(
                    crate::traits::Job::MigrationImportProcess { request_id },
                    crate::queue::worker::priority::LOW,
                )
                .await
            {
                return Err(JobError::Transient(format!(
                    "[MigrationImportProcess] 次回enqueue失敗: {e}"
                )));
            }
            Ok(())
        }
        ImportNextAction::Stop => Ok(()),
    }
}

async fn process_import(
    request_id: i64,
    pool: &PgPool,
    ctx: &JobContext,
) -> Result<ImportNextAction, JobError> {
    use crate::repository::{InsertFullParams, PgPostRepository, PostRepository};

    let repo = PgAtMigrationRepository::new(pool.clone());
    let Some(req) = repo
        .get(request_id)
        .await
        .map_err(|e| JobError::Transient(format!("[MigrationImportProcess] リクエスト取得失敗: {e}")))?
    else {
        tracing::warn!(
            "[MigrationImportProcess] request_id={} が見つかりません（終了）",
            request_id
        );
        return Ok(ImportNextAction::Stop);
    };

    if req.status != "importing_data" {
        tracing::info!(
            "[MigrationImportProcess] request_id={} は status={} のため終了",
            request_id,
            req.status
        );
        return Ok(ImportNextAction::Stop);
    }

    let actor_id = req.actor_id.ok_or_else(|| {
        JobError::Permanent("[MigrationImportProcess] actor_id が未確定です".to_string())
    })?;

    let follow_exec = ctx.follow_exec.as_ref().ok_or_else(|| {
        JobError::Transient("[MigrationImportProcess] FollowExecConfig 未設定".to_string())
    })?;
    let atp_service = &follow_exec.atp_service;
    let local_domain = &follow_exec.local_domain;

    let now = chrono::Utc::now();

    // ① atp_migration_records: 未取り込み分を1件ずつ実体化（posts or atp_records）
    if let Some((id, collection, rkey, cid, bytes)) = repo
        .claim_next_record(request_id)
        .await
        .map_err(|e| JobError::Transient(format!("[MigrationImportProcess] レコード取得失敗: {e}")))?
    {
        let _ = cid; // CIDは`commit_generic_record`/`commit_post_record`が再計算する（内容一致のはず）
        let value = crate::atp::decode_dagcbor_to_json(&bytes).map_err(|e| {
            JobError::Permanent(format!(
                "[MigrationImportProcess] レコードのCBORデコード失敗 (id={id}): {e}"
            ))
        })?;

        if collection == "app.bsky.feed.post" {
            let text = value
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let created_at = value
                .get("createdAt")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or(now);

            let post_id = crate::generate_snowflake_id(now);
            let ap_object_id = format!("https://{}/notes/{}", local_domain, post_id);
            let seiran_post_uuid = uuid::Uuid::new_v4().to_string();

            let posts_repo = PgPostRepository::new(pool.clone());
            posts_repo
                .insert_full(InsertFullParams {
                    id: post_id,
                    actor_id,
                    body: &text,
                    ap_object_id: &ap_object_id,
                    seiran_post_uuid: &seiran_post_uuid,
                    // リプライ/引用先の解決はスコープ外（既知の制限、docs/account_migration.md参照）。
                    reply_to_post_id: None,
                    quote_of_post_id: None,
                    created_at,
                    visibility: "public",
                    // 過去のBsky投稿の再取り込みであり新規投稿ではないため配送しない。
                    deliver_fedi: false,
                    deliver_bsky: false,
                    thread_root_post_id: None,
                    recipient_actor_ids: &[],
                    emoji_map: &serde_json::json!({}),
                    poll: None,
                    content_warning: None,
                    language: None,
                })
                .await
                .map_err(|e| {
                    JobError::Transient(format!(
                        "[MigrationImportProcess] posts INSERT失敗 (rkey={rkey}): {e}"
                    ))
                })?;

            // 画像/動画添付の復元。移行元DID（=移行後もDIDは不変）とblob CIDのみから
            // Bluesky CDN/動画パイプラインのURLを決定的に組み立てる既存ロジックを再利用する
            // （`atp_migration_blobs`側のblob取り込み順に依存しない）。
            if let Some(embed) = value.get("embed") {
                let attachments = crate::atp::parse_bsky_embed_attachments(embed, &req.source_did);
                for (position, att) in attachments.into_iter().enumerate() {
                    if let Err(e) = posts_repo
                        .attach_remote_media_url(
                            post_id,
                            &att.url,
                            Some(&att.mime_type),
                            att.thumbnail_url.as_deref(),
                            false,
                            att.is_gif,
                            position as i16,
                        )
                        .await
                    {
                        tracing::error!(
                            "[MigrationImportProcess] 添付URL保存失敗 (rkey={rkey}): {e}"
                        );
                    }
                }
            }

            atp_service
                .commit_post_record(actor_id, post_id, rkey.clone(), &value, "create", now)
                .await
                .map_err(|e| {
                    JobError::Transient(format!(
                        "[MigrationImportProcess] commit_post_record失敗 (rkey={rkey}): {e}"
                    ))
                })?;
        } else {
            atp_service
                .commit_generic_record(actor_id, collection.clone(), rkey.clone(), &value, "create", now)
                .await
                .map_err(|e| {
                    JobError::Transient(format!(
                        "[MigrationImportProcess] commit_generic_record失敗 (collection={collection}, rkey={rkey}): {e}"
                    ))
                })?;
        }

        repo.mark_record_imported(id, now).await.map_err(|e| {
            JobError::Transient(format!("[MigrationImportProcess] レコード取込済みマーク失敗: {e}"))
        })?;
        return Ok(ImportNextAction::Continue);
    }

    // ② at_migration_blobs: 未取り込み分を1件ずつPDS Aから取得しseiranのストレージへ保存
    if let Some((id, cid)) = repo
        .claim_next_blob(request_id)
        .await
        .map_err(|e| JobError::Transient(format!("[MigrationImportProcess] blob取得失敗: {e}")))?
    {
        let session = crate::atp::migration_client::AtpSession {
            did: req.source_did.clone(),
            handle: req.source_handle.clone(),
            access_jwt: req.source_access_jwt.clone().unwrap_or_default(),
            refresh_jwt: req.source_refresh_jwt.clone().unwrap_or_default(),
        };
        let resolved = crate::atp::did_resolve::resolve_stored_endpoint(&req.source_pds_endpoint)
            .await
            .map_err(|e| {
                JobError::Transient(format!("[MigrationImportProcess] PDSエンドポイント検証失敗: {e}"))
            })?;
        let bytes = crate::atp::migration_client::fetch_blob(&resolved, &session, &cid)
            .await
            .map_err(|e| {
                JobError::Transient(format!("[MigrationImportProcess] getBlob失敗 (cid={cid}): {e}"))
            })?;

        let encryption_key = ctx.encryption_key.clone().ok_or_else(|| {
            JobError::Transient("[MigrationImportProcess] encryption_key 未設定".to_string())
        })?;
        import_one_blob(pool, encryption_key, actor_id, &cid, &bytes)
            .await
            .map_err(|e| JobError::Transient(format!("[MigrationImportProcess] blob保存失敗: {e}")))?;

        repo.mark_blob_imported(id, now).await.map_err(|e| {
            JobError::Transient(format!("[MigrationImportProcess] blob取込済みマーク失敗: {e}"))
        })?;
        return Ok(ImportNextAction::Continue);
    }

    // ③ 両方尽きた: 移行元アカウント無効化（ベストエフォート）へ進める
    repo.set_status(request_id, "deactivating_source", now)
        .await
        .map_err(|e| JobError::Transient(format!("[MigrationImportProcess] ステータス更新失敗: {e}")))?;
    if let Err(e) = ctx
        .queue
        .enqueue(
            crate::traits::Job::MigrationDeactivateSource { request_id },
            crate::queue::worker::priority::LOW,
        )
        .await
    {
        tracing::error!(
            "[MigrationImportProcess] MigrationDeactivateSource enqueue失敗 (request_id={}): {}",
            request_id,
            e
        );
    }
    tracing::info!(
        "[MigrationImportProcess] request_id={} データ取り込み完了",
        request_id
    );
    Ok(ImportNextAction::Stop)
}

/// `store_uploaded_blob`（`seiran-api::handlers::xrpc::repo`、uploadBlob受け口）と同じ
/// 保存パイプライン（S3保存＋`atp_blobs` INSERT）の、転入フロー向け版。
/// クレート境界の都合（`seiran-api`側の関数はここから呼べない）で複製している。
async fn import_one_blob(
    pool: &PgPool,
    encryption_key: Vec<u8>,
    actor_id: i64,
    cid: &str,
    bytes: &[u8],
) -> Result<(), String> {
    use crate::repository::PgStorageProviderRepository;
    use crate::storage::{ext_for_mime_type, select_provider, sniff_mime_type, S3StorageClient};
    use sha2::{Digest, Sha256};

    let sha256_hex = hex::encode(Sha256::digest(bytes));

    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM atp_blobs WHERE sha256 = $1)")
        .bind(&sha256_hex)
        .fetch_one(pool)
        .await
        .map_err(|e| format!("既存チェック失敗: {e}"))?;
    if exists {
        return Ok(());
    }

    let mime_type = sniff_mime_type(bytes, "application/octet-stream");
    let storage_repo = PgStorageProviderRepository::new(pool.clone(), encryption_key);
    let provider = select_provider(&storage_repo, bytes.len() as i64)
        .await
        .map_err(|e| format!("ストレージプロバイダー選択失敗: {e}"))?;
    let ext = ext_for_mime_type(&mime_type);
    let storage_key = format!("blobs/{}.{}", uuid::Uuid::new_v4(), ext);
    let s3 = S3StorageClient::new(&provider);
    s3.put(&storage_key, bytes.to_vec(), &mime_type)
        .await
        .map_err(|e| format!("S3アップロード失敗: {e}"))?;

    let id = crate::generate_snowflake_id(chrono::Utc::now());
    sqlx::query(
        "INSERT INTO atp_blobs (id, actor_id, sha256, cid, mime_type, size, storage_provider_id, storage_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (sha256) DO NOTHING",
    )
    .bind(id)
    .bind(actor_id)
    .bind(&sha256_hex)
    .bind(cid)
    .bind(&mime_type)
    .bind(bytes.len() as i64)
    .bind(provider.id)
    .bind(&storage_key)
    .execute(pool)
    .await
    .map_err(|e| format!("atp_blobs INSERT失敗: {e}"))?;

    Ok(())
}

/// `Job::MigrationDeactivateSource` — 単発・ベストエフォート。失敗してもログのみで
/// `completed`へ進める（`auth.rs`の付随処理失敗時ログのみの原則と同じ）。
pub async fn handle_deactivate_source(request_id: i64, ctx: Arc<JobContext>) -> Result<(), JobError> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        return Err(JobError::Permanent(
            "[MigrationDeactivateSource] DB pool 未設定".to_string(),
        ));
    };

    let repo = PgAtMigrationRepository::new(pool.clone());
    let Some(req) = repo.get(request_id).await.map_err(|e| {
        JobError::Transient(format!("[MigrationDeactivateSource] リクエスト取得失敗: {e}"))
    })?
    else {
        return Ok(());
    };
    if req.status != "deactivating_source" {
        return Ok(());
    }

    let session = crate::atp::migration_client::AtpSession {
        did: req.source_did.clone(),
        handle: req.source_handle.clone(),
        access_jwt: req.source_access_jwt.clone().unwrap_or_default(),
        refresh_jwt: req.source_refresh_jwt.clone().unwrap_or_default(),
    };
    match crate::atp::did_resolve::resolve_stored_endpoint(&req.source_pds_endpoint).await {
        Ok(resolved) => {
            if let Err(e) = crate::atp::migration_client::deactivate_account(&resolved, &session).await
            {
                tracing::error!(
                    "[MigrationDeactivateSource] deactivateAccount失敗（続行、request_id={}）: {}",
                    request_id,
                    e
                );
            }
        }
        Err(e) => {
            tracing::error!(
                "[MigrationDeactivateSource] PDSエンドポイント検証失敗（続行、request_id={}）: {}",
                request_id,
                e
            );
        }
    }

    repo.set_status(request_id, "completed", chrono::Utc::now())
        .await
        .map_err(|e| {
            JobError::Transient(format!("[MigrationDeactivateSource] ステータス更新失敗: {e}"))
        })?;
    tracing::info!(
        "[MigrationDeactivateSource] request_id={} 完了（転入フロー完了）",
        request_id
    );
    Ok(())
}

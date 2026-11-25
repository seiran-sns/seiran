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
    fetch_repo_car, is_app_password_token, list_blobs, request_plc_operation_signature,
};
use crate::atp::migration_session::{call_with_refresh, source_session};
use crate::atp::mst_walk::decode_repo_records;
use crate::queue::worker::JobContext;
use crate::repository::{AtMigrationRepository, PgAtMigrationRepository, StagedRecord};
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

    let result = process_fetch_repo(request_id, pool).await;

    crate::advisory_lock::release(lock_conn, request_id).await;

    // 次のジョブはロックを放してから積む。握ったまま積むと、すぐに拾われた次のジョブが同じ
    // `request_id` のロックを取れずに「処理中」として何もせず終わり、転入が
    // `requesting_plc_signature` のまま止まる。
    if result? {
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

/// リポジトリを取得してステージングする。次のジョブ（確認コードの送付）を積むべきなら `true`。
async fn process_fetch_repo(request_id: i64, pool: &PgPool) -> Result<bool, JobError> {
    let repo = PgAtMigrationRepository::new(pool.clone());

    let Some(req) = repo.get(request_id).await.map_err(|e| {
        JobError::Transient(format!("[MigrationFetchRepo] リクエスト取得失敗: {e}"))
    })?
    else {
        tracing::warn!(
            "[MigrationFetchRepo] request_id={} が見つかりません（終了）",
            request_id
        );
        return Ok(false);
    };

    if req.status != "fetching_repo" {
        tracing::info!(
            "[MigrationFetchRepo] request_id={} は status={} のため終了",
            request_id,
            req.status
        );
        return Ok(false);
    }

    let now = chrono::Utc::now();
    let mut session = source_session(&req);

    // `start`時点で確定したPDS Aのエンドポイント文字列をそのまま使う（DID文書から
    // 再導出しない——`resolve_stored_endpoint`のドキュメントコメント参照）。
    // SSRF対策としてのIP検証だけは実行の都度やり直す（DNS rebinding対策）。
    let resolved = resolve_stored_endpoint(&req.source_pds_endpoint)
        .await
        .map_err(|e| {
            JobError::Transient(format!(
                "[MigrationFetchRepo] PDSエンドポイント検証失敗: {e}"
            ))
        })?;

    let car = call_with_refresh(pool, request_id, &resolved, &mut session, |s| {
        let resolved = &resolved;
        async move { fetch_repo_car(resolved, &s).await }
    })
    .await
    .map_err(|e| JobError::Transient(format!("[MigrationFetchRepo] getRepo失敗: {e}")))?;

    let records = match decode_repo_records(&car) {
        Ok(r) => r,
        Err(e) => {
            let msg =
                format!("[MigrationFetchRepo] CARデコード失敗（不正なリポジトリデータ）: {e}");
            repo.set_failed(request_id, "failed", &msg, now)
                .await
                .map_err(|e| {
                    JobError::Transient(format!("[MigrationFetchRepo] 失敗記録失敗: {e}"))
                })?;
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
        JobError::Transient(format!(
            "[MigrationFetchRepo] レコードステージング失敗: {e}"
        ))
    })?;

    let blob_cids = call_with_refresh(pool, request_id, &resolved, &mut session, |s| {
        let resolved = &resolved;
        async move { list_blobs(resolved, &s).await }
    })
    .await
    .map_err(|e| JobError::Transient(format!("[MigrationFetchRepo] listBlobs失敗: {e}")))?;
    let blob_count = blob_cids.len();
    repo.stage_blobs(request_id, &blob_cids)
        .await
        .map_err(|e| {
            JobError::Transient(format!("[MigrationFetchRepo] blobステージング失敗: {e}"))
        })?;

    // メールは`start`時点でPDS A由来のものを既に確定済み（転入フローは独自のメール実在確認を
    // 行わない）のため、`awaiting_seiran_email`を経由せず直接次のステップへ進む。
    repo.set_status(request_id, "requesting_plc_signature", now)
        .await
        .map_err(|e| {
            JobError::Transient(format!("[MigrationFetchRepo] ステータス更新失敗: {e}"))
        })?;

    tracing::info!(
        "[MigrationFetchRepo] request_id={} 完了 (records={}, blobs={})",
        request_id,
        staged_count,
        blob_count,
    );
    Ok(true)
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
        JobError::Transient(format!(
            "[MigrationRequestPlcSignature] リクエスト取得失敗: {e}"
        ))
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

    let mut session = source_session(&req);
    // アプリパスワードのセッションでは署名要求が権限不足で拒まれ、何度やり直しても通らない。
    // 呼ぶ前に分かるならそこで、分からなければ拒まれた時点で、本来のパスワードの入れ直し待ちにする。
    if is_app_password_token(&session.access_jwt) {
        return await_source_password(pool, request_id).await;
    }

    let resolved = resolve_stored_endpoint(&req.source_pds_endpoint)
        .await
        .map_err(|e| {
            JobError::Transient(format!(
                "[MigrationRequestPlcSignature] PDSエンドポイント検証失敗: {e}"
            ))
        })?;

    let result = call_with_refresh(pool, request_id, &resolved, &mut session, |s| {
        let resolved = &resolved;
        async move { request_plc_operation_signature(resolved, &s).await }
    })
    .await;
    match result {
        Ok(()) => {}
        Err(e) if e.is_insufficient_scope() => {
            return await_source_password(pool, request_id).await;
        }
        Err(e) => {
            return Err(JobError::Transient(format!(
                "[MigrationRequestPlcSignature] requestPlcOperationSignature失敗: {e}"
            )));
        }
    }

    repo.set_status(request_id, "awaiting_plc_token", chrono::Utc::now())
        .await
        .map_err(|e| {
            JobError::Transient(format!(
                "[MigrationRequestPlcSignature] ステータス更新失敗: {e}"
            ))
        })?;

    tracing::info!(
        "[MigrationRequestPlcSignature] request_id={} 完了（PDS A登録メールへ確認コード送付済み）",
        request_id
    );

    Ok(())
}

/// 移行元のセッションがアプリパスワードのもので PLC 操作に署名させられないので、
/// 本来のパスワードの入れ直し待ち（`awaiting_source_password`）にする。入力されたら
/// `submit-source-password` がセッションを取り直し、署名要求からやり直す。
async fn await_source_password(pool: &PgPool, request_id: i64) -> Result<(), JobError> {
    let moved = crate::repository::at_migration::transition_status(
        pool,
        request_id,
        "requesting_plc_signature",
        "awaiting_source_password",
    )
    .await
    .map_err(|e| {
        JobError::Transient(format!(
            "[MigrationRequestPlcSignature] ステータス更新失敗: {e}"
        ))
    })?;
    if moved {
        tracing::info!(
            "[MigrationRequestPlcSignature] request_id={} はアプリパスワードのセッションのため、移行元のパスワードの入れ直しを待ちます",
            request_id
        );
    }
    Ok(())
}

/// `process_import`が返す「unlock後にやるべきこと」（`jobs::follow_import`と同じ設計）。
enum ImportNextAction {
    Continue,
    /// 今は進められるものが無い（取り直しの時刻を待つリポジトリ・blob しか残っていない）。
    /// この時間の後に自分を積み直す。
    RetryAfter(std::time::Duration),
    Stop,
}

/// 転入元から一時的に取れなかったもの（リポジトリの取り直し・blob）を後回しにする時間。
/// 失敗を重ねるたびに倍にし、上限で止める（転入元が止まっている間、叩き続けないため）。
/// 期限は設けず、取れるまで（利用者が打ち切るまで）続ける。
fn retry_delay_secs(attempts: i32) -> i64 {
    const BASE_SECS: i64 = 60;
    const MAX_SECS: i64 = 30 * 60;
    BASE_SECS
        .saturating_mul(1i64 << attempts.clamp(0, 10))
        .min(MAX_SECS)
}

/// 転入元に無い blob（取り直しても取れない）。それ以外の失敗は転入元が戻れば取れうるので、
/// 取り直し続ける。
fn is_missing_blob(e: &crate::atp::migration_client::MigrationClientError) -> bool {
    matches!(
        e,
        crate::atp::migration_client::MigrationClientError::Xrpc { status, code, .. }
            if *status == 404 || code == "BlobNotFound"
    )
}

/// `Job::MigrationImportProcess` — 自己再enqueue型。PLC 提出・アカウント確定の後に、利用者が
/// アカウントを使っている裏で進める。`importing_data`ではリポジトリを取り直してから
/// `at_migration_records`を、`importing_blobs`では`at_migration_blobs`を1件ずつ実体化し、
/// 尽きたら次の状態へ進める。転入元から取れないものは後回しにし、それしか残っていなければ
/// その時刻に自分を積み直す（ワーカーのリトライに任せると5回で破棄され、取り込みが止まる）。
/// `follow_import`と同じ`request_id`単位advisory lock。
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

    // 続きを積むときだけ、次に動く予定を残す。予定を過ぎても動いていなければ、連鎖が途切れた
    // （積んだジョブが失われた）とみなして定期処理が積み直す（`list_stalled_job_migrations`）。
    // 止まるとき（`Stop`）は書かない: 知らない状態を見て止まった場合（自分より新しい版が足した
    // 状態を、無停止更新の間に拾ったとき）にも予定を消してしまうため。取り込みを終えたかは
    // 状態で分かる（取り込み以外へ進めば積み直しの対象にならない）。
    let next_run_at = match &result {
        Ok(ImportNextAction::Continue) => Some(chrono::Utc::now()),
        Ok(ImportNextAction::RetryAfter(delay)) => Some(
            chrono::Utc::now()
                + chrono::Duration::from_std(*delay).unwrap_or_else(|_| chrono::Duration::zero()),
        ),
        Ok(ImportNextAction::Stop) | Err(_) => None,
    };
    if let Some(next_run_at) = next_run_at {
        if let Err(e) =
            crate::repository::at_migration::set_import_next_run_at(pool, request_id, next_run_at)
                .await
        {
            tracing::warn!(
                "[MigrationImportProcess] request_id={} 次の予定の記録失敗: {}",
                request_id,
                e
            );
        }
    }

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
        ImportNextAction::RetryAfter(delay) => ctx
            .queue
            .enqueue_retry(
                crate::traits::Job::MigrationImportProcess { request_id },
                crate::queue::worker::priority::LOW,
                0,
                delay,
            )
            .await
            .map_err(|e| {
                JobError::Transient(format!(
                    "[MigrationImportProcess] 取り直しの再投入失敗: {e}"
                ))
            }),
        ImportNextAction::Stop => Ok(()),
    }
}

async fn process_import(
    request_id: i64,
    pool: &PgPool,
    ctx: &JobContext,
) -> Result<ImportNextAction, JobError> {
    let repo = PgAtMigrationRepository::new(pool.clone());
    let Some(req) = repo.get(request_id).await.map_err(|e| {
        JobError::Transient(format!("[MigrationImportProcess] リクエスト取得失敗: {e}"))
    })?
    else {
        tracing::warn!(
            "[MigrationImportProcess] request_id={} が見つかりません（終了）",
            request_id
        );
        return Ok(ImportNextAction::Stop);
    };

    if req.status != "importing_data" && req.status != "importing_blobs" {
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

    let now = chrono::Utc::now();
    let ic = ImportContext {
        pool,
        req: &req,
        actor_id,
        follow_exec,
        encryption_key: ctx.encryption_key.clone(),
        now,
    };

    if req.status == "importing_data" {
        return import_next_record(&ic, &repo, ctx).await;
    }
    import_next_blob(&ic, &repo, ctx).await
}

/// `importing_data`: リポジトリの取り直しの時刻が来ていれば取り直し、未取り込みのレコードを
/// 1件実体化する（posts or atp_records）。取り直しに失敗しても、手元のスナップショットの
/// レコードは先に取り込み続ける。レコードが尽き、取り直しも済んだら`importing_blobs`へ進め、
/// フォロー関係の復元を始める。
async fn import_next_record(
    ic: &ImportContext<'_>,
    repo: &PgAtMigrationRepository,
    ctx: &JobContext,
) -> Result<ImportNextAction, JobError> {
    let request_id = ic.req.id;
    let refetch_due = ic.req.repo_refetched_at.is_none()
        && ic.req.repo_refetch_next_at.is_none_or(|t| t <= ic.now);
    if refetch_due {
        match refetch_repo(ic).await {
            Ok(summary) => tracing::info!(
                "[MigrationImportProcess] request_id={} リポジトリを取り直しました（追加・変更 {} 件、取り込み前に消された {} 件）",
                request_id,
                summary.upserted,
                summary.removed
            ),
            Err(e) => {
                let delay = retry_delay_secs(ic.req.repo_refetch_attempts);
                tracing::warn!(
                    "[MigrationImportProcess] request_id={} リポジトリの取り直しに失敗、{} 秒後に再試行: {}",
                    request_id,
                    delay,
                    e
                );
                crate::repository::at_migration::defer_repo_refetch(ic.pool, request_id, &e, delay)
                    .await
                    .map_err(|e| {
                        JobError::Transient(format!("[MigrationImportProcess] {e}"))
                    })?;
            }
        }
    }

    if let Some((id, collection, rkey, _cid, bytes)) =
        repo.claim_next_record(request_id).await.map_err(|e| {
            JobError::Transient(format!("[MigrationImportProcess] レコード取得失敗: {e}"))
        })?
    {
        // CIDは`import_record`が再計算する（内容一致のはず）
        import_staged_record(ic, id, collection, rkey, &bytes).await?;
        repo.mark_record_imported(id, ic.now).await.map_err(|e| {
            JobError::Transient(format!(
                "[MigrationImportProcess] レコード取込済みマーク失敗: {e}"
            ))
        })?;
        return Ok(ImportNextAction::Continue);
    }

    // スナップショットの分は取り込み終えたが、取り直しがまだ済んでいない（転入元が応答しない）。
    // 取り直しの時刻まで待つ。
    // 上で取り直した結果を反映した最新の行で判定する。
    let latest = repo
        .get(request_id)
        .await
        .map_err(|e| JobError::Transient(format!("[MigrationImportProcess] {e}")))?
        .ok_or_else(|| JobError::Permanent("[MigrationImportProcess] 行が消えました".into()))?;
    if latest.repo_refetched_at.is_none() {
        let next_at = latest.repo_refetch_next_at.unwrap_or(ic.now);
        return Ok(ImportNextAction::RetryAfter(delay_until(next_at)));
    }

    announce_activation(ic).await?;
    crate::repository::at_migration::start_blob_import(ic.pool, request_id)
        .await
        .map_err(|e| {
            JobError::Transient(format!("[MigrationImportProcess] ステータス更新失敗: {e}"))
        })?;
    // フォロー関係の復元（`follows`テーブルへの反映）は`at_migration_requests.status`とは
    // 独立した結果整合処理として並行して進める（リモートアクター解決に時間がかかっても
    // 転入完了自体をブロックしないため）。
    if let Err(e) = ctx
        .queue
        .enqueue(
            crate::traits::Job::MigrationImportFollows { request_id },
            crate::queue::worker::priority::LOW,
        )
        .await
    {
        tracing::error!(
            "[MigrationImportProcess] MigrationImportFollows enqueue失敗 (request_id={}): {}",
            request_id,
            e
        );
    }
    tracing::info!(
        "[MigrationImportProcess] request_id={} レコードの取り込み完了、blob の取り込みへ",
        request_id
    );
    Ok(ImportNextAction::Continue)
}

/// レコードを取り込み終えたアカウントを、Relay/AppView に有効なアカウントとして知らせる
/// （`#account`→`#identity`→`#sync`）。公式 PDS で転入先が`importRepo`の後に`activateAccount`
/// を呼ぶのに当たる。続けて、取り込み中に利用者が seiran でした書き込み（有効化まで積んで
/// いた）を流す。リポジトリが揃ってから知らせるのは、レコードの欠けたリポジトリを最新の
/// 状態として伝えないため。blob はリポジトリの中身ではないので待たない。途中で止まっても、
/// やり直しでイベントをもう一度送り、残りの書き込みを流すだけで害は無い。
async fn announce_activation(ic: &ImportContext<'_>) -> Result<(), JobError> {
    let actor = ic
        .follow_exec
        .actors
        .find_by_id(ic.actor_id)
        .await
        .map_err(|e| {
            JobError::Transient(format!("[MigrationImportProcess] アクター取得失敗: {e}"))
        })?
        .ok_or_else(|| {
            JobError::Permanent("[MigrationImportProcess] アクターが見つかりません".to_string())
        })?;
    let handle = format!(
        "{}.{}",
        crate::username::to_atp_username(&actor.username),
        ic.follow_exec.local_domain
    );
    ic.follow_exec
        .atp_service
        .activate_migrated_account(ic.actor_id, &handle, chrono::Utc::now())
        .await
        .map_err(|e| {
            JobError::Transient(format!(
                "[MigrationImportProcess] アカウントの有効化失敗: {e}"
            ))
        })
}

/// `importing_blobs`: 取り込み待ちの blob を1件、転入元から取得して seiran のストレージへ
/// 保存する。転入元に無い blob は諦め、一時的に取れない blob は後回しにして他を先に取る。
/// 取り終えるか諦めたら（利用者の打ち切りを含む）、転入元の無効化へ進める。
async fn import_next_blob(
    ic: &ImportContext<'_>,
    repo: &PgAtMigrationRepository,
    ctx: &JobContext,
) -> Result<ImportNextAction, JobError> {
    use crate::repository::at_migration as migration_repo;
    let request_id = ic.req.id;
    let db_err = |e: sqlx::Error| JobError::Transient(format!("[MigrationImportProcess] {e}"));

    if let Some((id, cid)) = repo.claim_next_blob(request_id).await.map_err(db_err)? {
        let bytes = match fetch_source_blob(ic.pool, ic.req, &cid).await {
            Ok(bytes) => bytes,
            Err(e) if is_missing_blob(&e) => {
                tracing::warn!(
                    "[MigrationImportProcess] request_id={} 転入元に無い blob を諦めます (cid={}): {}",
                    request_id,
                    cid,
                    e
                );
                migration_repo::give_up_blob(ic.pool, id, &e.to_string())
                    .await
                    .map_err(db_err)?;
                return Ok(ImportNextAction::Continue);
            }
            Err(e) => {
                defer_blob(ic.pool, id, &format!("getBlob失敗: {e}")).await?;
                return Ok(ImportNextAction::Continue);
            }
        };
        let encryption_key = ctx.encryption_key.clone().ok_or_else(|| {
            JobError::Transient("[MigrationImportProcess] encryption_key 未設定".to_string())
        })?;
        if let Err(e) = import_one_blob(ic.pool, encryption_key, ic.actor_id, &cid, &bytes).await {
            defer_blob(ic.pool, id, &format!("blob保存失敗: {e}")).await?;
            return Ok(ImportNextAction::Continue);
        }
        repo.mark_blob_imported(id, ic.now).await.map_err(|e| {
            JobError::Transient(format!(
                "[MigrationImportProcess] blob取込済みマーク失敗: {e}"
            ))
        })?;
        return Ok(ImportNextAction::Continue);
    }

    let (pending, next_at) = migration_repo::pending_blobs(ic.pool, request_id)
        .await
        .map_err(db_err)?;
    if pending == 0 {
        reapply_imported_profile(ic).await;
        return finish_import(request_id, repo, ctx, ic.now).await;
    }
    // 取り直しの時刻が来た blob が無い。最も早いものまで待つ。
    Ok(ImportNextAction::RetryAfter(delay_until(
        next_at.unwrap_or(ic.now),
    )))
}

/// `at` までの待ち時間（過ぎていても数秒は空ける）。
fn delay_until(at: chrono::DateTime<chrono::Utc>) -> std::time::Duration {
    (at - chrono::Utc::now())
        .to_std()
        .unwrap_or_default()
        .max(std::time::Duration::from_secs(5))
}

/// PLC 提出後にリポジトリ全体と blob の一覧を取り直し、スナップショット（`fetching_repo`）より
/// 後の変更を取り込み対象に反映する。スナップショットから PLC 提出までの間に利用者が転入元で
/// した投稿等を取りこぼさないため。PLC 提出後も転入元はアカウントを有効なまま持っている
/// （無効化は blob の取り込みの後）ので取れる。
async fn refetch_repo(
    ic: &ImportContext<'_>,
) -> Result<crate::repository::at_migration::RefetchSummary, String> {
    let req = ic.req;
    let mut session = source_session(req);
    let resolved = resolve_stored_endpoint(&req.source_pds_endpoint)
        .await
        .map_err(|e| format!("PDSエンドポイント検証失敗: {e}"))?;
    let car = call_with_refresh(ic.pool, req.id, &resolved, &mut session, |s| {
        let resolved = &resolved;
        async move { fetch_repo_car(resolved, &s).await }
    })
    .await
    .map_err(|e| format!("getRepo失敗: {e}"))?;
    let records: Vec<StagedRecord> = decode_repo_records(&car)
        .map_err(|e| format!("CARデコード失敗: {e}"))?
        .into_iter()
        .map(|r| StagedRecord {
            collection: r.collection,
            rkey: r.rkey,
            cid: r.cid.to_string(),
            bytes: r.bytes,
        })
        .collect();
    let blob_cids = call_with_refresh(ic.pool, req.id, &resolved, &mut session, |s| {
        let resolved = &resolved;
        async move { list_blobs(resolved, &s).await }
    })
    .await
    .map_err(|e| format!("listBlobs失敗: {e}"))?;
    let repo = PgAtMigrationRepository::new(ic.pool.clone());
    repo.stage_blobs(req.id, &blob_cids)
        .await
        .map_err(|e| format!("blobステージング失敗: {e}"))?;
    crate::repository::at_migration::apply_refetched_repo(ic.pool, req.id, &records)
        .await
        .map_err(|e| format!("取り直したレコードの反映失敗: {e}"))
}

/// 一時的に取れなかった blob を、失敗の回数に応じた時間だけ後回しにする。
async fn defer_blob(pool: &PgPool, blob_id: i64, error: &str) -> Result<(), JobError> {
    let attempts: i32 = crate::repository::at_migration::blob_attempts(pool, blob_id)
        .await
        .map_err(|e| JobError::Transient(format!("[MigrationImportProcess] {e}")))?;
    let delay = retry_delay_secs(attempts);
    tracing::warn!(
        "[MigrationImportProcess] blob id={} を {} 秒後に取り直します: {}",
        blob_id,
        delay,
        error
    );
    crate::repository::at_migration::defer_blob(pool, blob_id, error, delay)
        .await
        .map_err(|e| JobError::Transient(format!("[MigrationImportProcess] {e}")))
}

/// レコード取り込み1件分の共通文脈。
struct ImportContext<'a> {
    pool: &'a PgPool,
    req: &'a crate::repository::AtMigrationRequestRow,
    actor_id: i64,
    follow_exec: &'a crate::queue::worker::FollowExecConfig,
    /// blob の保存先ストレージプロバイダーの選択に使う（`JobContext::encryption_key`）。
    encryption_key: Option<Vec<u8>>,
    now: chrono::DateTime<chrono::Utc>,
}

/// ステージング済みレコード1件を実体化する。投稿は`posts`行を作ってからATPリポジトリへ、
/// それ以外はATPリポジトリへそのままコミットする。
async fn import_staged_record(
    ic: &ImportContext<'_>,
    id: i64,
    collection: String,
    rkey: String,
    bytes: &[u8],
) -> Result<(), JobError> {
    let value = crate::atp::decode_dagcbor_to_json(bytes).map_err(|e| {
        JobError::Permanent(format!(
            "[MigrationImportProcess] レコードのCBORデコード失敗 (id={id}): {e}"
        ))
    })?;

    // 取り込むレコードは firehose に流さない（`AtpCommitService::import_record`）。取り込み後の
    // 状態は、レコードを取り込み終えたときの`announce_activation`でまとめて知らせる。
    let post_id = if collection == "app.bsky.feed.post" {
        Some(materialize_migrated_post(ic, &rkey, &value).await?)
    } else {
        None
    };
    ic.follow_exec
        .atp_service
        .import_record(ic.actor_id, collection.clone(), rkey.clone(), &value, post_id, ic.now)
        .await
        .map_err(|e| {
            JobError::Transient(format!(
                "[MigrationImportProcess] レコードのコミット失敗 (collection={collection}, rkey={rkey}): {e}"
            ))
        })?;
    if collection == "app.bsky.actor.profile" && rkey == "self" {
        sync_imported_profile(ic, &value).await;
    }
    Ok(())
}

/// 取り込んだプロフィールを seiran のローカルアクターへ反映する（ATP → actors）。片方だけだと、
/// 次に seiran でプロフィールを編集したとき、空の`actors`を元に`profile/self`がコミットされて
/// Bluesky 側の表示名やアバターが消える。アバター・バナーの blob は取り込みの最後に回るので、
/// ここで先に転入元から取ってくる（取れなければ、blob を取り終えた時点で
/// `reapply_imported_profile`がもう一度当てはめる）。失敗しても取り込みは止めない。
async fn sync_imported_profile(ic: &ImportContext<'_>, value: &serde_json::Value) {
    let request_id = ic.req.id;
    for key in ["avatar", "banner"] {
        let Some(cid) = value
            .get(key)
            .and_then(|b| b.get("ref"))
            .and_then(|r| r.get("$link"))
            .and_then(|c| c.as_str())
        else {
            continue;
        };
        if let Err(e) = prefetch_profile_blob(ic, cid).await {
            tracing::warn!(
                "[MigrationImportProcess] request_id={} プロフィールの{}を先に取れませんでした（blob の取り込みで取り直す）: {}",
                request_id,
                key,
                e
            );
        }
    }
    let Some(user_id) = ic.req.user_id else {
        return;
    };
    if let Err(e) = crate::atp::profile_sync::apply_profile_record(
        ic.pool,
        user_id,
        value,
        crate::atp::profile_sync::BlobOwner::Any,
    )
    .await
    {
        tracing::error!(
            "[MigrationImportProcess] request_id={} プロフィールの actors への反映失敗: {}",
            request_id,
            e
        );
    }
}

/// プロフィールのアバター・バナーの blob を、取り込みの順番を待たずに転入元から取って保存する。
async fn prefetch_profile_blob(ic: &ImportContext<'_>, cid: &str) -> Result<(), String> {
    use crate::repository::at_migration as migration_repo;
    let Some(blob_id) = migration_repo::pending_blob_id_by_cid(ic.pool, ic.req.id, cid)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let bytes = fetch_source_blob(ic.pool, ic.req, cid)
        .await
        .map_err(|e| format!("getBlob失敗: {e}"))?;
    let encryption_key = ic
        .encryption_key
        .clone()
        .ok_or_else(|| "encryption_key 未設定".to_string())?;
    import_one_blob(ic.pool, encryption_key, ic.actor_id, cid, &bytes).await?;
    PgAtMigrationRepository::new(ic.pool.clone())
        .mark_blob_imported(blob_id, ic.now)
        .await
        .map_err(|e| e.to_string())
}

/// blob を取り終えた時点で、取り込んだプロフィールをもう一度 actors へ当てはめる（先に取れ
/// なかったアバター・バナーを反映するため）。
async fn reapply_imported_profile(ic: &ImportContext<'_>) {
    let Some(user_id) = ic.req.user_id else {
        return;
    };
    if let Err(e) =
        reapply_imported_profile_to_actor(ic.pool, ic.req.id, ic.actor_id, user_id).await
    {
        tracing::error!(
            "[MigrationImportProcess] request_id={} プロフィールの actors への再反映失敗: {}",
            ic.req.id,
            e
        );
    }
}

/// 転入で取り込んだプロフィールを、ローカルアクターへ当てはめる（ATP → actors）。取り込んだ後に
/// seiran 側でプロフィールが編集されていれば（`profile/self`の CID が取り込んだものと変わって
/// いれば）当てはめない: 編集は両方の世界に反映済みで、古い内容に戻してしまうため。
/// 当てはめたら `true`。
pub async fn reapply_imported_profile_to_actor(
    pool: &PgPool,
    request_id: i64,
    actor_id: i64,
    user_id: i64,
) -> Result<bool, String> {
    let Some((bytes, current_cid)) =
        crate::repository::at_migration::imported_profile_and_current_cid(
            pool, request_id, actor_id,
        )
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(false);
    };
    let value = crate::atp::decode_dagcbor_to_json(&bytes).map_err(|e| e.to_string())?;
    let (_, imported_cid) =
        crate::atp::repo::encode_generic_record(&value).map_err(|e| e.to_string())?;
    if current_cid.as_deref() != Some(crate::atp::repo::cid_to_string(&imported_cid).as_str()) {
        return Ok(false);
    }
    crate::atp::profile_sync::apply_profile_record(
        pool,
        user_id,
        &value,
        crate::atp::profile_sync::BlobOwner::Any,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(true)
}

/// 転入済みのアカウントに、取り込んだプロフィールを当てはめ直す（起動時）。取り込みが
/// プロフィールを actors へ反映していなかった間に転入したアカウントの表示名・自己紹介・
/// アバター・バナーを埋めるため。seiran 側で編集済みのアカウントは対象外
/// （`reapply_imported_profile_to_actor`）。何度実行しても同じ結果になる。
pub async fn backfill_migrated_profiles(pool: &PgPool) {
    let accounts = match crate::repository::at_migration::list_migrated_accounts(pool).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[startup] 転入済みアカウントの取得に失敗: {}", e);
            return;
        }
    };
    let mut applied = 0usize;
    for (request_id, actor_id, user_id) in accounts {
        match reapply_imported_profile_to_actor(pool, request_id, actor_id, user_id).await {
            Ok(true) => applied += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(
                "[startup] 転入 id={} のプロフィールの当てはめ直しに失敗: {}",
                request_id,
                e
            ),
        }
    }
    if applied > 0 {
        tracing::info!(
            "[startup] 転入済みアカウント {} 件に取り込んだプロフィールを当てはめ直しました",
            applied
        );
    }
}

/// 転入した投稿レコードに対応する`posts`行を用意し、その id を返す。
async fn materialize_migrated_post(
    ic: &ImportContext<'_>,
    rkey: &str,
    value: &serde_json::Value,
) -> Result<i64, JobError> {
    use crate::repository::{InsertFullParams, PgPostRepository, PostRepository};

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
        .unwrap_or(ic.now);

    // `seiranPost`拡張オブジェクト（他seiranサーバー間の投稿完全再現、#237）。転入元も
    // seiranであれば、この投稿がATP標準フィールドだけでは再現できない情報（CW・投票・
    // 絵文字マップ・URLカードの申告値・公開範囲）を持っている。リモートseiranポスト受信
    // （`seiran-atp-repo::firehose`の`save_bsky_post`呼び出し前後）と同じ優先順位で
    // 標準フィールドを上書きする。
    let seiran_post_ext = crate::seiran_post::SeiranPost::extract(value);
    let body_text = seiran_post_ext
        .as_ref()
        .map(|sp| sp.body.clone())
        .unwrap_or(text);
    let emoji_map = seiran_post_ext
        .as_ref()
        .map(|sp| sp.emoji_map.clone())
        .unwrap_or_else(|| serde_json::json!({}));
    let content_warning = seiran_post_ext
        .as_ref()
        .and_then(|sp| sp.content_warning.clone());
    let poll = seiran_post_ext.as_ref().and_then(|sp| sp.poll.clone());
    let visibility = seiran_post_ext
        .as_ref()
        .map(|sp| sp.visibility.clone())
        .unwrap_or_else(|| "public".to_string());
    let language = seiran_post_ext.as_ref().and_then(|sp| sp.language.clone());

    let posts_repo = PgPostRepository::new(ic.pool.clone());

    // 移行元DID（=移行後もDIDは不変）は、転入前からseiranがリモートキャッシュとして
    // 既に観測済み（firehose購読・プロフィール参照等）のことがある——`actors`行自体も
    // その場合は新規作成せず変換（UPDATE）して再利用する設計（`migration.rs`の
    // `submit_plc_token`参照）。同じ理由で`posts`側も`at_uri`（DIDが不変なので
    // 転入前後で同一値になる）が既存キャッシュ行と衝突しうる。新規行を作らず
    // 既存行を再利用しないと`posts_at_uri_key`の一意制約違反でリトライが延々続く。
    let at_uri = format!("at://{}/app.bsky.feed.post/{}", ic.req.source_did, rkey);
    let existing_post_id = posts_repo.find_id_by_at_uri(&at_uri).await.map_err(|e| {
        JobError::Transient(format!(
            "[MigrationImportProcess] 既存post確認失敗 (rkey={rkey}): {e}"
        ))
    })?;
    if let Some(existing_id) = existing_post_id {
        return Ok(existing_id);
    }

    // id は元の投稿日時から採番する。タイムラインは id 順に並ぶため、取り込み時刻で採番すると
    // 過去の投稿が新着としてローカルタイムラインの先頭を占め、クライアントの取りこぼし補完
    // （再接続時の先頭ページ再取得）でも新着として扱われてしまう。
    let post_id = crate::generate_snowflake_id(created_at);
    let ap_object_id = format!("https://{}/notes/{}", ic.follow_exec.local_domain, post_id);
    let seiran_post_uuid = uuid::Uuid::new_v4().to_string();

    posts_repo
        .insert_full(InsertFullParams {
            id: post_id,
            actor_id: ic.actor_id,
            body: &body_text,
            ap_object_id: &ap_object_id,
            seiran_post_uuid: &seiran_post_uuid,
            // リプライ/引用先の解決はスコープ外（既知の制限、docs/account_migration.md参照）。
            reply_to_post_id: None,
            quote_of_post_id: None,
            created_at,
            visibility: &visibility,
            // 過去のBsky投稿の再取り込みであり新規投稿ではないため配送しない。
            deliver_fedi: false,
            deliver_bsky: false,
            thread_root_post_id: None,
            recipient_actor_ids: &[],
            emoji_map: &emoji_map,
            poll: poll.as_ref(),
            content_warning: content_warning.as_deref(),
            language: language.as_deref(),
            scheduled_post: None,
        })
        .await
        .map_err(|e| {
            JobError::Transient(format!(
                "[MigrationImportProcess] posts INSERT失敗 (rkey={rkey}): {e}"
            ))
        })?;

    if let Some(sp) = &seiran_post_ext {
        if !sp.link_cards.is_empty() {
            crate::seiran_post::insert_seiran_post_link_cards(ic.pool, post_id, &sp.link_cards)
                .await;
        }
    }

    // 画像/動画添付の復元。移行元DIDとblob CIDのみからBluesky CDN/動画パイプラインの
    // URLを決定的に組み立てる既存ロジックを再利用する（`atp_migration_blobs`側の
    // blob取り込み順に依存しない）。
    if let Some(embed) = value.get("embed") {
        let attachments = crate::atp::parse_bsky_embed_attachments(embed, &ic.req.source_did);
        // 転入元で閲覧注意のラベルを付けていた投稿は、添付を NSFW 指定のまま引き継ぐ。
        let is_sensitive = crate::atp::record_has_media_self_label(value);
        for (position, att) in attachments.into_iter().enumerate() {
            if let Err(e) = posts_repo
                .attach_remote_media_url(
                    post_id,
                    &crate::repository::RemoteAttachment {
                        url: &att.url,
                        mime_type: Some(&att.mime_type),
                        thumbnail_url: att.thumbnail_url.as_deref(),
                        is_sensitive,
                        is_gif: att.is_gif,
                        position: position as i16,
                    },
                )
                .await
            {
                tracing::error!("[MigrationImportProcess] 添付URL保存失敗 (rkey={rkey}): {e}");
            }
        }
    }

    Ok(post_id)
}

/// 転入元PDSから blob を1件取得する。
async fn fetch_source_blob(
    pool: &PgPool,
    req: &crate::repository::AtMigrationRequestRow,
    cid: &str,
) -> Result<Vec<u8>, crate::atp::migration_client::MigrationClientError> {
    let mut session = source_session(req);
    let resolved = crate::atp::did_resolve::resolve_stored_endpoint(&req.source_pds_endpoint)
        .await
        .map_err(|e| crate::atp::migration_client::MigrationClientError::Resolve(e.to_string()))?;
    call_with_refresh(pool, req.id, &resolved, &mut session, |s| {
        let resolved = &resolved;
        async move { crate::atp::migration_client::fetch_blob(resolved, &s, cid).await }
    })
    .await
}

/// blob を取り終えるか諦めたら、転入元アカウントの無効化へ進める（無効化された転入元は
/// getBlob に応答しなくなるため、blob が済むまで待つ）。
async fn finish_import(
    request_id: i64,
    repo: &PgAtMigrationRepository,
    ctx: &JobContext,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<ImportNextAction, JobError> {
    repo.set_status(request_id, "deactivating_source", now)
        .await
        .map_err(|e| {
            JobError::Transient(format!("[MigrationImportProcess] ステータス更新失敗: {e}"))
        })?;
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
        "[MigrationImportProcess] request_id={} blob の取り込み完了、転入元の無効化へ",
        request_id
    );
    Ok(ImportNextAction::Stop)
}

/// `store_uploaded_blob`（`seiran-api::handlers::xrpc::repo`、uploadBlob受け口）と同じ
/// 保存パイプライン（S3保存＋`MediaFileRepository::upsert`）の、転入フロー向け版。
/// クレート境界の都合（`seiran-api`側のS3アップロード部分はここから呼べない）で
/// アップロード処理自体は複製しているが、DB登録は共通の`upsert`を経由するため重複しない。
/// `cid`は転入元PDSから取得済みの値だが保存には使わない（sha256から決定論的に
/// 再構築できるため、`media_files`にcid専用カラムは持たない）。
async fn import_one_blob(
    pool: &PgPool,
    encryption_key: Vec<u8>,
    actor_id: i64,
    _cid: &str,
    bytes: &[u8],
) -> Result<(), String> {
    use crate::repository::{
        CreateMediaFile, MediaFileRepository, PgMediaFileRepository, PgStorageProviderRepository,
    };
    use crate::storage::{ext_for_mime_type, select_provider, sniff_mime_type, S3StorageClient};
    use sha2::{Digest, Sha256};

    let sha256_hex = hex::encode(Sha256::digest(bytes));
    let media_files = PgMediaFileRepository::new(pool.clone());

    // 既に media_files にあれば S3 への重複保存を避ける（事前チェック）。この確認から
    // 下の upsert までの間に別のリクエストが同じ内容を先に登録していても、upsert が
    // アトミックに吸収するため安全に収束する。
    if let Some(existing) = media_files
        .find_by_sha256(&sha256_hex)
        .await
        .map_err(|e| format!("既存チェック失敗: {e}"))?
    {
        media_files
            .touch_last_uploaded_at(existing.id)
            .await
            .map_err(|e| format!("last_uploaded_at更新失敗: {e}"))?;
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
    media_files
        .upsert(CreateMediaFile {
            id,
            storage_provider_id: provider.id,
            sha256: sha256_hex,
            blurhash: None,
            size: bytes.len() as i64,
            width: None,
            height: None,
            mime_type,
            storage_key,
            duration_ms: None,
            thumbnail_key: None,
            uploaded_by_actor_id: Some(actor_id),
            is_animated_image: false,
        })
        .await
        .map(|_| ())
        .map_err(|e| format!("media_files INSERT失敗: {e}"))
}

/// `Job::MigrationDeactivateSource` — 単発・ベストエフォート。失敗しても`completed`へ進める。
/// 結果は`source_deactivated_at`/`source_deactivate_error`に残す（`auth.rs`の付随処理失敗時ログのみの原則と同じ）。
pub async fn handle_deactivate_source(
    request_id: i64,
    ctx: Arc<JobContext>,
) -> Result<(), JobError> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        return Err(JobError::Permanent(
            "[MigrationDeactivateSource] DB pool 未設定".to_string(),
        ));
    };

    let repo = PgAtMigrationRepository::new(pool.clone());
    let Some(req) = repo.get(request_id).await.map_err(|e| {
        JobError::Transient(format!(
            "[MigrationDeactivateSource] リクエスト取得失敗: {e}"
        ))
    })?
    else {
        return Ok(());
    };
    if req.status != "deactivating_source" {
        return Ok(());
    }

    let mut session = source_session(&req);
    // PDS 経由だと PLC 送信後は必ず BadJwtSignature になるので、まず entryway へ直接送る
    // （`account_management_endpoint`）。entryway で失敗し、送り先が保存済みの PDS と
    // 違うなら、PDS へ一度だけ送り直す（XRPC を受けないホストの did:web を
    // サービス DID にしている自前 PDS 向け）。
    let endpoint = crate::atp::migration_client::account_management_endpoint(
        req.source_refresh_jwt.as_deref(),
        &req.source_pds_endpoint,
    );
    let mut outcome = deactivate_source_at(pool, request_id, &endpoint, &mut session).await;
    if outcome.is_err() && endpoint != req.source_pds_endpoint {
        tracing::warn!(
            "[MigrationDeactivateSource] entryway での無効化に失敗、PDS へ送り直す（request_id={}）: {}",
            request_id,
            outcome.as_ref().err().map(String::as_str).unwrap_or_default()
        );
        outcome =
            deactivate_source_at(pool, request_id, &req.source_pds_endpoint, &mut session).await;
    }
    if let Err(e) = &outcome {
        tracing::error!(
            "[MigrationDeactivateSource] {}（続行、request_id={}）",
            e,
            request_id
        );
    }
    if let Err(e) = crate::repository::at_migration::record_source_deactivation(
        pool,
        request_id,
        outcome.as_ref().map(|_| ()).map_err(String::as_str),
    )
    .await
    {
        tracing::error!(
            "[MigrationDeactivateSource] 無効化結果の記録失敗（続行、request_id={}）: {}",
            request_id,
            e
        );
    }

    repo.set_status(request_id, "completed", chrono::Utc::now())
        .await
        .map_err(|e| {
            JobError::Transient(format!(
                "[MigrationDeactivateSource] ステータス更新失敗: {e}"
            ))
        })?;
    tracing::info!(
        "[MigrationDeactivateSource] request_id={} 完了（転入フロー完了）",
        request_id
    );
    Ok(())
}

/// `endpoint` へ `deactivateAccount` を送る（ExpiredToken なら同じ送り先で取り直して1回だけ再試行）。
async fn deactivate_source_at(
    pool: &PgPool,
    request_id: i64,
    endpoint: &str,
    session: &mut crate::atp::migration_client::AtpSession,
) -> Result<(), String> {
    let resolved = crate::atp::did_resolve::resolve_stored_endpoint(endpoint)
        .await
        .map_err(|e| format!("PDSエンドポイント検証失敗（{endpoint}）: {e}"))?;
    call_with_refresh(pool, request_id, &resolved, session, |s| {
        let resolved = &resolved;
        async move { crate::atp::migration_client::deactivate_account(resolved, &s).await }
    })
    .await
    .map_err(|e| format!("deactivateAccount失敗（{endpoint}）: {e}"))
}

/// `Job::MigrationImportFollows` — 自己再enqueue型。取り込み済み`app.bsky.graph.follow`
/// レコード（`at_migration_records`、`imported_at`設定済み）を1件ずつ`follows`テーブルへ
/// 反映する（リモートアクター解決込み）。`at_migration_requests.status`とは独立して動作し、
/// `deactivating_source`/`completed`への遷移を待たない結果整合処理。
/// レート制限は適用しない（新規フォローではなく既存関係の復元のため）。
///
/// advisory lockのキーは`-request_id`（負数）を使う。`handle_import_process`等の
/// 他の転入ジョブ群は同一`request_id`（正数）をキーに使っており、このジョブは
/// `MigrationImportProcess`がレコードを取り込み終えた時点でそれらと同時に起動されるため、同じキーだと
/// 片方が`try_acquire`に失敗して黙って終了してしまう（advisory lockはジョブ種別を
/// 区別しないグローバルなキー空間のため）。
pub async fn handle_import_follows(request_id: i64, ctx: Arc<JobContext>) -> Result<(), JobError> {
    let Some(pool) = ctx.db_pool.as_ref() else {
        return Err(JobError::Permanent(
            "[MigrationImportFollows] DB pool 未設定".to_string(),
        ));
    };

    let Some(lock_conn) = crate::advisory_lock::try_acquire(pool, -request_id)
        .await
        .map_err(JobError::Transient)?
    else {
        tracing::info!(
            "[MigrationImportFollows] request_id={} は既に別のジョブが処理中のためスキップ",
            request_id
        );
        return Ok(());
    };

    let result = process_import_follows(request_id, pool, &ctx).await;

    crate::advisory_lock::release(lock_conn, -request_id).await;

    match result? {
        ImportNextAction::Continue => {
            if let Err(e) = ctx
                .queue
                .enqueue(
                    crate::traits::Job::MigrationImportFollows { request_id },
                    crate::queue::worker::priority::LOW,
                )
                .await
            {
                return Err(JobError::Transient(format!(
                    "[MigrationImportFollows] 次回enqueue失敗: {e}"
                )));
            }
            Ok(())
        }
        ImportNextAction::RetryAfter(delay) => ctx
            .queue
            .enqueue_retry(
                crate::traits::Job::MigrationImportFollows { request_id },
                crate::queue::worker::priority::LOW,
                0,
                delay,
            )
            .await
            .map_err(|e| JobError::Transient(format!("[MigrationImportFollows] 再投入失敗: {e}"))),
        ImportNextAction::Stop => Ok(()),
    }
}

async fn process_import_follows(
    request_id: i64,
    pool: &PgPool,
    ctx: &JobContext,
) -> Result<ImportNextAction, JobError> {
    let repo = PgAtMigrationRepository::new(pool.clone());
    let Some(req) = repo.get(request_id).await.map_err(|e| {
        JobError::Transient(format!("[MigrationImportFollows] リクエスト取得失敗: {e}"))
    })?
    else {
        tracing::warn!(
            "[MigrationImportFollows] request_id={} が見つかりません（終了）",
            request_id
        );
        return Ok(ImportNextAction::Stop);
    };
    let Some(actor_id) = req.actor_id else {
        // まだactor_id未確定（submitting_plc未到達）ならこのジョブが起動されることは
        // 無いはずだが、念のため。
        return Ok(ImportNextAction::Stop);
    };

    let follow_exec = ctx.follow_exec.as_ref().ok_or_else(|| {
        JobError::Transient("[MigrationImportFollows] FollowExecConfig 未設定".to_string())
    })?;

    let Some((id, rkey, bytes)) = repo
        .claim_next_follow_record(request_id)
        .await
        .map_err(|e| {
            JobError::Transient(format!("[MigrationImportFollows] レコード取得失敗: {e}"))
        })?
    else {
        tracing::info!(
            "[MigrationImportFollows] request_id={} フォロー関係復元完了",
            request_id
        );
        return Ok(ImportNextAction::Stop);
    };

    let now = chrono::Utc::now();
    let value = crate::atp::decode_dagcbor_to_json(&bytes).map_err(|e| {
        JobError::Permanent(format!(
            "[MigrationImportFollows] レコードのCBORデコード失敗 (id={id}): {e}"
        ))
    })?;
    let Some(subject_did) = value.get("subject").and_then(|v| v.as_str()) else {
        tracing::warn!(
            "[MigrationImportFollows] subjectが無いためスキップ (rkey={})",
            rkey
        );
        repo.mark_follow_materialized(id, now).await.map_err(|e| {
            JobError::Transient(format!("[MigrationImportFollows] 復元済みマーク失敗: {e}"))
        })?;
        return Ok(ImportNextAction::Continue);
    };

    match crate::follow_exec::link_bsky_follow_with_existing_rkey(
        subject_did,
        actor_id,
        &rkey,
        pool,
        &ctx.ap_client,
        &ctx.queue,
        follow_exec,
    )
    .await
    {
        Ok(_) => {}
        Err(e) => {
            // 個々のフォロー対象の解決失敗（相手アカウント削除済み・ブロック関係等）は
            // 全体を止めず、このレコードだけスキップして続行する。
            tracing::warn!(
                "[MigrationImportFollows] フォロー復元失敗 (rkey={}, subject={}): {}",
                rkey,
                subject_did,
                e
            );
        }
    }

    repo.mark_follow_materialized(id, now).await.map_err(|e| {
        JobError::Transient(format!("[MigrationImportFollows] 復元済みマーク失敗: {e}"))
    })?;
    Ok(ImportNextAction::Continue)
}

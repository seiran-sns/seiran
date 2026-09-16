//! アカウント単位PLCローテーションキーへのバックフィル（転出元API対応の前提、Phase A）。
//!
//! ジェネシス作成の全ローカルアカウントは現在、サーバー全体共有の単一ローテーション
//! キー（`secrets.toml`）を使っている。これを「アカウント単位鍵（主）＋共有鍵（副＝
//! recovery用）」の2鍵構成へ移行する一括処理。対象の選定は
//! `repository::RotationKeyBackfillRepository`（既存DID転入済みアカウントは対象外、
//! 理由は同ファイル参照）。
//!
//! **これは自動実行しない。** 管理者が明示的に起動する操作としてのみ呼ばれることを
//! 前提にしている（呼び出し元の管理用エンドポイント側でも同様の注意を徹底すること）。
//! `dry_run=true`では実際にはplc.directoryへ提出せず、各アカウントについて
//! 「何を提出する予定か」をログ出力するだけに留める。

use std::sync::Arc;

use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;

use crate::atp::plc::{
    fetch_current_plc_doc_and_prev, p256_to_did_key, prepare_plc_rotation_update,
    submit_plc_operation_raw,
};
use crate::repository::RotationKeyBackfillRepository;

pub struct BackfillReport {
    pub processed: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub dry_run: bool,
}

/// 全対象アカウントを1件ずつ処理する。失敗した行はスキップして次に進み、
/// `at_rotation_key_pem`はNULLのまま残る（再実行時に自然に再対象化される）。
///
/// `mark_backfilled`を呼ばないdry-runでは`next_candidate`が状態で前進しないため、
/// このループ自身が`after_id`カーソルで進行を管理する（呼ばないと同じ1件を無限に
/// 返し続けてしまう——実機で発見）。失敗した行もこのカーソルで読み飛ばし、
/// 1件の失敗が全体のスキャンを止めないようにする。
pub async fn run(
    repo: Arc<dyn RotationKeyBackfillRepository>,
    http_client: &reqwest::Client,
    server_shared_key: &SigningKey,
    dry_run: bool,
) -> BackfillReport {
    let mut processed = 0u64;
    let mut succeeded = 0u64;
    let mut failed = 0u64;
    let mut after_id: Option<i64> = None;

    loop {
        let candidate = match repo.next_candidate(after_id).await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => {
                tracing::error!("[RotationKeyBackfill] 対象取得失敗: {}", e);
                break;
            }
        };
        let (actor_id, did) = candidate;
        after_id = Some(actor_id);
        processed += 1;

        match process_one(&did, http_client, server_shared_key, dry_run).await {
            Ok(rotation_key_pem) => {
                if dry_run {
                    tracing::info!(
                        "[RotationKeyBackfill][dry-run] actor_id={} did={} 提出予定の内容を確認済み（提出はしない）",
                        actor_id,
                        did
                    );
                    succeeded += 1;
                    continue;
                }
                if let Err(e) = repo.mark_backfilled(actor_id, &rotation_key_pem).await {
                    tracing::error!(
                        "[RotationKeyBackfill] actor_id={} did={} バックフィル完了マーク失敗（PLC提出は成功済み!): {}",
                        actor_id,
                        did,
                        e
                    );
                    failed += 1;
                    continue;
                }
                tracing::info!(
                    "[RotationKeyBackfill] actor_id={} did={} バックフィル成功",
                    actor_id,
                    did
                );
                succeeded += 1;
            }
            Err(e) => {
                tracing::error!(
                    "[RotationKeyBackfill] actor_id={} did={} 失敗（次回再試行対象として残す）: {}",
                    actor_id,
                    did,
                    e
                );
                failed += 1;
            }
        }
    }

    BackfillReport {
        processed,
        succeeded,
        failed,
        dry_run,
    }
}

async fn process_one(
    did: &str,
    http_client: &reqwest::Client,
    server_shared_key: &SigningKey,
    dry_run: bool,
) -> Result<String, String> {
    let (current_data, prev) = fetch_current_plc_doc_and_prev(did, http_client)
        .await
        .map_err(|e| format!("現在のDIDドキュメント取得失敗: {e}"))?;

    let new_account_key = SigningKey::random(&mut argon2::password_hash::rand_core::OsRng);
    let new_account_did_key = p256_to_did_key(new_account_key.verifying_key());
    let new_rotation_keys = vec![
        new_account_did_key.clone(),
        p256_to_did_key(server_shared_key.verifying_key()),
    ];

    let operation = prepare_plc_rotation_update(
        &current_data,
        &prev,
        new_rotation_keys,
        None,
        None,
        None,
        server_shared_key,
    )
    .map_err(|e| format!("更新オペレーション生成失敗: {e}"))?;

    if dry_run {
        tracing::info!(
            "[RotationKeyBackfill][dry-run] did={} 新rotationKeys[0]={} prev={} operation={}",
            did,
            new_account_did_key,
            prev,
            operation
        );
        return Ok(String::new());
    }

    submit_plc_operation_raw(did, &operation, http_client)
        .await
        .map_err(|e| format!("plc.directory提出失敗: {e}"))?;

    let rotation_key_pem = new_account_key
        .to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
        .map_err(|e| format!("鍵PEM変換失敗: {e}"))?
        .to_string();
    Ok(rotation_key_pem)
}

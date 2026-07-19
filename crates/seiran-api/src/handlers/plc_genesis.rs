//! `did:plc` 発行の共通処理（register / setup で共有）。
//!
//! 1. DID 確定（ローカル計算のみ）→ 2. Cloudflare TXT セット → 3. plc.directory へ送信、
//!    を最大3回リトライする。リトライ時は genesis を再生成（新しいランダム鍵 → 別の署名 →
//!    別の DID）し、前回セットした TXT を削除してから新 DID 用の TXT を置き直す。
//!    DB 書き込みは呼び出し側がこの関数の成功後に行う（失敗時に孤立レコードが残らないため）。

use p256::ecdsa::SigningKey;

use seiran_common::atp::{prepare_plc_genesis, submit_plc_genesis};

use crate::error::ApiError;
use crate::AppState;

/// `did:plc` 発行の結果: `(at_did, at_signing_key_pem, cloudflare_txt_record_id)`。
pub type PlcGenesisResult = (String, String, Option<String>);

/// `did:plc` を発行する（最大3回リトライ）。`log_prefix` はログの `[register]`/`[setup]` 等の
/// タグに使う。
pub async fn register_plc_did(
    state: &AppState,
    username: &str,
    rotation_key: &SigningKey,
    log_prefix: &str,
) -> Result<PlcGenesisResult, ApiError> {
    let mut prev_cf_id: Option<String> = None;
    let mut attempt = 0u8;

    loop {
        attempt += 1;

        // リトライ時: 前回の TXT を削除してから新しい genesis を使う
        if let (Some(cf), Some(old_id)) = (&state.cloudflare, prev_cf_id.take()) {
            let _ = cf.delete_txt_record(&old_id).await;
        }

        // 1. DID 確定（ローカル計算のみ）
        let genesis = match prepare_plc_genesis(username, &state.local_domain, rotation_key) {
            Ok(g) => g,
            Err(e) => {
                tracing::error!("[{}] genesis 準備失敗 (試行 {}/3): {}", log_prefix, attempt, e);
                if attempt >= 3 {
                    return Err(ApiError::Internal("did:plc genesis 準備エラー".to_string()));
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };

        // 2. Cloudflare TXT セット（plc.directory 送信より先に配置）
        let new_cf_id = if let Some(cf) = &state.cloudflare {
            let handle = format!("{}.{}", username, state.local_domain);
            match cf.set_atproto_txt(&handle, &genesis.did).await {
                Ok(id) => {
                    tracing::info!("[{}] Cloudflare TXT セット完了: _atproto.{}", log_prefix, handle);
                    Some(id)
                }
                Err(e) => {
                    tracing::error!("[{}] Cloudflare TXT セット失敗（登録は継続）: {}", log_prefix, e);
                    None
                }
            }
        } else {
            None
        };

        // 3. plc.directory へ送信
        match submit_plc_genesis(&genesis, &state.http_client).await {
            Ok(()) => return Ok((genesis.did, genesis.signing_key_pem, new_cf_id)),
            Err(e) => {
                tracing::error!("[{}] did:plc 送信失敗 (試行 {}/3): {}", log_prefix, attempt, e);
                prev_cf_id = new_cf_id;
                if attempt >= 3 {
                    if let (Some(cf), Some(id)) = (state.cloudflare.clone(), prev_cf_id) {
                        let log_prefix = log_prefix.to_string();
                        tokio::spawn(async move {
                            let _ = cf.delete_txt_record(&id).await;
                            tracing::error!("[{}] did:plc 失敗のため TXT 削除", log_prefix);
                        });
                    }
                    tracing::error!("[{}] did:plc 登録失敗（3回）: {}", log_prefix, e);
                    return Err(ApiError::Internal("did:plc 登録エラー（3回失敗）".to_string()));
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}

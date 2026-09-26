//! アカウント単位PLCローテーションキーへのバックフィル（転出元API対応の前提、Phase A）。
//! 管理者が明示的に起動する操作。自動実行（デプロイ時・起動時等）には絶対に組み込まない。

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use seiran_common::atp::plc::signing_key_from_pem;
use seiran_common::repository::PgRotationKeyBackfillRepository;

use crate::error::ApiError;
use crate::AppState;

#[derive(Deserialize)]
pub struct BackfillRequest {
    /// trueなら実際にはplc.directoryへ提出せず、提出予定の内容をログ出力するだけに留める。
    #[serde(default)]
    pub dry_run: bool,
    /// dry_run=falseの場合のみ必須。誤操作防止のための明示的な確認フラグ
    /// （管理者権限に加えて、この操作の重大さを踏まえた二重チェック）。
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Serialize)]
pub struct BackfillResponse {
    pub processed: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub dry_run: bool,
}

pub async fn run_rotation_key_backfill(
    State(state): State<AppState>,
    Json(req): Json<BackfillRequest>,
) -> Result<Json<BackfillResponse>, ApiError> {
    if !req.dry_run && !req.confirm {
        return Err(ApiError::BadRequest(
            "ROTATION_KEY_BACKFILL_CONFIRM_REQUIRED".to_string(),
        ));
    }

    let server_shared_key =
        signing_key_from_pem(&state.secrets.atproto_private_key_pem).map_err(|e| {
            tracing::error!("[admin:rotation-key-backfill] 共有鍵ロード失敗: {}", e);
            ApiError::Internal("ATP鍵ロードエラー".to_string())
        })?;

    let repo = std::sync::Arc::new(PgRotationKeyBackfillRepository::new(state.db.clone()));

    tracing::warn!(
        "[admin:rotation-key-backfill] 開始 (dry_run={})",
        req.dry_run
    );
    let report = seiran_common::rotation_key_backfill::run(
        repo,
        &state.http_client,
        &server_shared_key,
        req.dry_run,
    )
    .await;
    tracing::warn!(
        "[admin:rotation-key-backfill] 完了 processed={} succeeded={} failed={} dry_run={}",
        report.processed,
        report.succeeded,
        report.failed,
        report.dry_run
    );

    Ok(Json(BackfillResponse {
        processed: report.processed,
        succeeded: report.succeeded,
        failed: report.failed,
        dry_run: report.dry_run,
    }))
}

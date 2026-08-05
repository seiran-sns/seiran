//! ヘルスチェック（docs/code_audit_2026-08-05.md R-9）。
//!
//! 「APIプロセスは起動しているがDBプールが枯渇/切断している」状態を外形監視から
//! 区別できるようにする。認証不要（監視システムからの疎通確認用）。

use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;

use crate::AppState;

#[derive(Serialize)]
pub struct HealthResponse {
    status: &'static str,
}

/// `GET /health`
/// DBへ`SELECT 1`を発行できるかで判定する。失敗時は503（起動直後の一時的な
/// 未接続とプロセス生存の"200だが実は死んでいる"を区別するため、200固定にはしない）。
pub async fn health(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.db)
        .await
    {
        Ok(_) => (StatusCode::OK, Json(HealthResponse { status: "ok" })),
        Err(e) => {
            tracing::error!("[health] DB疎通確認失敗: {}", e);
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HealthResponse {
                    status: "db_unreachable",
                }),
            )
        }
    }
}

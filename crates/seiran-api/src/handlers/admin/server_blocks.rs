//! 管理画面「サーバーブロック」タブ: 拒否サーバーの一覧の読み書き。
//! 拒否サーバーの扱いは`seiran_common::denied_host`を参照。

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use seiran_common::repository::denied_host;

use crate::error::ApiError;
use crate::AppState;

#[derive(Serialize)]
pub struct DeniedHostsResponse {
    pub hosts: Vec<String>,
}

#[derive(Deserialize)]
pub struct UpdateDeniedHostsRequest {
    /// textarea の内容（1行1ドメイン）。
    pub text: String,
}

/// GET /api/admin/denied-hosts
pub async fn get_denied_hosts(
    State(state): State<AppState>,
) -> Result<Json<DeniedHostsResponse>, ApiError> {
    let hosts = denied_host::list_hosts(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(DeniedHostsResponse { hosts }))
}

/// PUT /api/admin/denied-hosts
///
/// 正規化（`normalize_host_list`）した一覧で丸ごと置き換え、その結果を返す。
pub async fn update_denied_hosts(
    State(state): State<AppState>,
    Json(body): Json<UpdateDeniedHostsRequest>,
) -> Result<Json<DeniedHostsResponse>, ApiError> {
    let hosts = seiran_common::denied_host::normalize_host_list(&body.text);
    denied_host::replace_hosts(&state.db, &hosts)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    seiran_common::denied_host::invalidate_cache();
    if !hosts.is_empty() {
        // ATP アクターの所属サーバーは PDS ホストで判定するので、取り込み対象のうち未解決の
        // ものを裏で解決しておく（既存の投稿もグローバル・ハッシュタグから外れるように）。
        let pool = state.db.clone();
        tokio::spawn(async move {
            seiran_common::denied_host::backfill_pds_hosts(&pool).await;
        });
    }
    Ok(Json(DeniedHostsResponse { hosts }))
}

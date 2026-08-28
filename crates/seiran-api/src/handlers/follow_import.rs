//! フォローインポート（設定画面から改行区切りのID一覧を貼り付けて一括フォロー）。
//!
//! 隠し仕様として各行をカンマ区切りで分割し1列目のみを識別子として読む（Misskeyの
//! フォローエクスポートCSVはヘッダ無しの `id,withRepliesフラグ` 形式のため、そのまま
//! 対応できる）。この解析はここ（バックエンド）で行い、フロントエンドは生テキストを
//! そのまま送るだけにする。
//!
//! ## API
//! - `POST /api/account/follow-import` — インポート対象一覧を受け取り、自己再enqueue型
//!   ジョブ（`Job::FollowImportProcess`）を開始する
//! - `GET  /api/account/follow-import` — 直近リクエストの進捗を返す
//! - `POST /api/account/follow-import/cancel` — 実行中リクエストをキャンセルする

use axum::{extract::State, Json};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use seiran_common::generate_snowflake_id;

use crate::error::ApiError;
use crate::middleware::AuthedUser;
use crate::AppState;

/// 1リクエストあたりのインポート対象上限。
const MAX_FOLLOW_IMPORT_ITEMS: usize = 20_000;

#[derive(Deserialize)]
pub struct StartImportRequest {
    pub text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowImportStartResponse {
    pub request_id: i64,
    pub total: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowImportStatusResponse {
    /// `idle` / `running` / `completed` / `cancelled`
    pub status: String,
    pub total: i32,
    pub processed: i64,
    pub succeeded: i64,
    /// 呼び出し前から既にフォロー関係が存在していたため、新規フォローが成立しなかった件数
    /// （`succeeded` とは別枠）。
    pub already_following: i64,
    pub failed: i64,
}

/// 改行区切りの各行をカンマ区切りで分割し1列目のみを識別子として読む（隠し仕様）。
/// trim後に空文字の行は除外する。
fn parse_import_targets(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split(',').next().unwrap_or("").trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

pub async fn start_import(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<StartImportRequest>,
) -> Result<Json<FollowImportStartResponse>, ApiError> {
    let targets = parse_import_targets(&req.text);
    if targets.is_empty() {
        return Err(ApiError::BadRequest(
            "インポート対象がありません".to_owned(),
        ));
    }
    if targets.len() > MAX_FOLLOW_IMPORT_ITEMS {
        return Err(ApiError::BadRequest(format!(
            "インポート対象が多すぎます（最大{}件）",
            MAX_FOLLOW_IMPORT_ITEMS
        )));
    }

    let active = state
        .follow_imports
        .find_active_for_actor(user.actor_id)
        .await
        .map_err(|e| ApiError::Internal(format!("実行中インポート確認失敗: {}", e)))?;
    if active.is_some() {
        return Err(ApiError::Conflict("既に実行中のインポートがあります"));
    }

    let request_id = generate_snowflake_id(Utc::now());
    let total = targets.len();
    state
        .follow_imports
        .create_request(request_id, user.actor_id, &targets, Utc::now())
        .await
        .map_err(|e| ApiError::Internal(format!("インポートリクエスト作成失敗: {}", e)))?;

    state.enqueue_follow_import_process(request_id).await;

    Ok(Json(FollowImportStartResponse { request_id, total }))
}

pub async fn get_status(
    user: AuthedUser,
    State(state): State<AppState>,
) -> Result<Json<FollowImportStatusResponse>, ApiError> {
    let progress = state
        .follow_imports
        .find_latest_for_actor(user.actor_id)
        .await
        .map_err(|e| ApiError::Internal(format!("進捗取得失敗: {}", e)))?;

    let resp = match progress {
        Some(p) => FollowImportStatusResponse {
            status: p.status,
            total: p.total,
            processed: p.succeeded + p.already_following + p.failed,
            succeeded: p.succeeded,
            already_following: p.already_following,
            failed: p.failed,
        },
        None => FollowImportStatusResponse {
            status: "idle".to_string(),
            total: 0,
            processed: 0,
            succeeded: 0,
            already_following: 0,
            failed: 0,
        },
    };
    Ok(Json(resp))
}

pub async fn cancel_import(
    user: AuthedUser,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(request_id) = state
        .follow_imports
        .find_active_for_actor(user.actor_id)
        .await
        .map_err(|e| ApiError::Internal(format!("実行中インポート確認失敗: {}", e)))?
    else {
        return Err(ApiError::NotFound("実行中のインポートがありません"));
    };

    let cancelled = state
        .follow_imports
        .cancel(request_id, user.actor_id, Utc::now())
        .await
        .map_err(|e| ApiError::Internal(format!("キャンセル失敗: {}", e)))?;

    if !cancelled {
        return Err(ApiError::NotFound("実行中のインポートがありません"));
    }

    Ok(Json(serde_json::json!({"status": "cancelled"})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_usernames() {
        assert_eq!(
            parse_import_targets("alice\nbob\ncarol"),
            vec!["alice", "bob", "carol"]
        );
    }

    #[test]
    fn extracts_first_csv_column_only() {
        // Misskeyフォローエクスポート形式: `id,withRepliesフラグ`（ヘッダ無し）
        assert_eq!(
            parse_import_targets("9wg8k3xyz1,false\n9wg8k3xyz2,true"),
            vec!["9wg8k3xyz1", "9wg8k3xyz2"]
        );
    }

    #[test]
    fn skips_empty_lines_and_trims_whitespace() {
        assert_eq!(
            parse_import_targets("alice\n\n  bob  \n\n"),
            vec!["alice", "bob"]
        );
    }

    #[test]
    fn empty_text_yields_no_targets() {
        assert!(parse_import_targets("").is_empty());
        assert!(parse_import_targets("\n\n\n").is_empty());
    }

    #[test]
    fn handles_crlf_line_endings() {
        assert_eq!(
            parse_import_targets("alice\r\nbob\r\n"),
            vec!["alice", "bob"]
        );
    }
}

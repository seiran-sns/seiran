//! URL・AT URI・ユーザーIDを取り込み、SPA内の遷移先へ変換する薄いラッパー。
//!
//! 解決ロジック本体（URL分類・アクター/投稿解決）は`seiran_common::link_target`へ移動済み
//! （bio/profile_fields内リンクのバックグラウンド解決`jobs::link_resolve`とも共有するため。
//! 挙動不変のリファクタ）。ここでは`AppState`から`ResolveContext`を組み立て、結果を
//! フロントの遷移先パスへ変換するだけ。Misskey互換API `POST /api/ap/show`
//! （`handlers::misskey::endpoints::ap_show`、`misskey_dart`の`MisskeyAp.show`、Ariaの
//! 「ほかのアカウントで開く」機能）は`{type, object}`（本家Misskey準拠、`Note`/`User`の
//! フルオブジェクト）へ別途変換する。

use axum::{extract::State, Json};
pub use seiran_common::link_target::ResolvedTarget;
use seiran_common::link_target::{self, ResolveContext, ResolveError};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::middleware::AuthedUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct OpenTargetRequest {
    pub target: String,
}

#[derive(Serialize)]
pub struct OpenTargetResponse {
    pub path: String,
    pub kind: &'static str,
}

pub async fn open_target(
    State(state): State<AppState>,
    _user: AuthedUser,
    Json(req): Json<OpenTargetRequest>,
) -> Result<Json<OpenTargetResponse>, ApiError> {
    let resolved = resolve_open_target(&state, &req.target).await?;
    Ok(Json(match resolved {
        ResolvedTarget::Actor(actor) => {
            let acct = seiran_common::username::actor_handle(
                &actor.username,
                &actor.domain,
                &actor.actor_type,
            );
            OpenTargetResponse {
                path: format!("/{}", acct),
                kind: "actor",
            }
        }
        ResolvedTarget::Post(post_id) => OpenTargetResponse {
            path: format!("/notes/{post_id}"),
            kind: "post",
        },
    }))
}

/// `target`（URL・AT URI・ユーザーID等）を解決する。ローカルDBに無ければ取り込み（フェッチ
/// ＋保存）まで行う。呼び出し元（SPA向け`open_target`・Misskey互換`ap_show`）が結果を
/// それぞれの応答形へ変換する。
pub async fn resolve_open_target(
    state: &AppState,
    target: &str,
) -> Result<ResolvedTarget, ApiError> {
    let parsed = link_target::parse_target(target).ok_or_else(invalid_open_target_error)?;
    let ctx = ResolveContext {
        actors: state.actors.as_ref(),
        posts: state.posts.as_ref(),
        ap_client: &state.ap_client,
        job_queue: &state.job_queue,
        db_pool: &state.db,
        local_domain: state.local_domain.as_str(),
        system_signing_key: state.system_signing_key(),
    };
    link_target::resolve_target(&ctx, parsed)
        .await
        .map_err(|e| match e {
            ResolveError::ImportPending => {
                ApiError::ServiceUnavailable("OPEN_TARGET_IMPORT_PENDING")
            }
            ResolveError::Upstream(msg) => ApiError::BadGateway(msg),
            ResolveError::Internal(msg) => ApiError::Internal(msg),
            ResolveError::Invalid => invalid_open_target_error(),
        })
}

fn invalid_open_target_error() -> ApiError {
    ApiError::BadRequest("INVALID_OPEN_TARGET".to_string())
}

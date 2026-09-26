//! フォロー対象文字列（ローカルユーザー名 / `@user@domain` / `https://...` / `did:...` /
//! ATPハンドル）を解決し、必要ならリモートアクターをDBへupsertして返す薄いラッパー。
//!
//! `follows.rs` の follow_local/follow_bsky/follow_fedi はフォロー関係の作成まで
//! 一気に行うが、リスト機能のメンバー追加（`handlers::lists`）はフォロー関係を
//! 作らずアクター解決だけを必要とするため、この関数として切り出す。
//!
//! 解決ロジック本体は`seiran_common::target_resolve`へ移動済み（`open_target`のリンク解決・
//! `link_resolve`ジョブからも同じ処理が必要になったため。挙動不変のリファクタ）。ここでは
//! `AppState`から`TargetResolveContext`を組み立てるだけ。

use seiran_common::repository::Actor;
use seiran_common::target_resolve::{self, TargetResolveContext};
use seiran_common::ApError;

use crate::error::ApiError;
use crate::AppState;

/// `actor_a`/`actor_b` のいずれかがもう一方をブロックしていれば `Forbidden` を返す。
/// フォロー作成・リプライ作成・リアクション作成の書き込みガードで共通に使う
/// （seiranのブロックはBsky準拠＝相互完全非表示のため、方向を問わず拒否する）。
pub async fn check_not_blocked(
    state: &AppState,
    actor_a: i64,
    actor_b: i64,
) -> Result<(), ApiError> {
    let (is_blocking, is_blocked_by) = state
        .blocks
        .find_relationship(actor_a, actor_b)
        .await
        .map_err(|e| ApiError::Internal(format!("ブロック関係取得失敗: {}", e)))?;
    if is_blocking || is_blocked_by {
        return Err(ApiError::Forbidden("BLOCKED"));
    }
    Ok(())
}

pub async fn resolve_and_upsert_target(state: &AppState, target: &str) -> Result<Actor, ApError> {
    let ctx = TargetResolveContext {
        actors: state.actors.as_ref(),
        ap_client: &state.ap_client,
        local_domain: state.local_domain.as_str(),
        system_signing_key: state.system_signing_key(),
    };
    target_resolve::resolve_and_upsert_target(&ctx, target).await
}

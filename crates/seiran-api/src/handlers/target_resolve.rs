//! フォロー対象文字列（ローカルユーザー名 / `@user@domain` / `https://...` / `did:...` /
//! ATPハンドル）を解決し、必要ならリモートアクターをDBへupsertして返す共通ロジック。
//!
//! `follows.rs` の follow_local/follow_bsky/follow_fedi はフォロー関係の作成まで
//! 一気に行うが、リスト機能のメンバー追加（`handlers::lists`）はフォロー関係を
//! 作らずアクター解決だけを必要とするため、この関数として切り出す。

use seiran_common::atp::fetch_bsky_profile;
use seiran_common::repository::Actor;
use seiran_common::{generate_snowflake_id, ApError};

use crate::error::ApiError;
use crate::AppState;

/// `actor_a`/`actor_b` のいずれかがもう一方をブロックしていれば `Forbidden` を返す。
/// フォロー作成・リプライ作成・リアクション作成の書き込みガードで共通に使う
/// （seiranのブロックはBsky準拠＝相互完全非表示のため、方向を問わず拒否する）。
pub async fn check_not_blocked(state: &AppState, actor_a: i64, actor_b: i64) -> Result<(), ApiError> {
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
    let t = target.trim().trim_start_matches('@');

    if t.starts_with("https://") || t.starts_with("http://") {
        return resolve_fedi(state, t).await;
    }

    if t.starts_with("did:") {
        return resolve_bsky(state, t).await;
    }

    if t.contains('.') && !t.contains('@') {
        return resolve_bsky(state, t).await;
    }

    let parts: Vec<&str> = t.splitn(2, '@').collect();
    if parts.len() == 1 || (parts.len() == 2 && parts[1] == state.local_domain) {
        let username = parts[0];
        return state
            .actors
            .find_by_username_domain(username, &state.local_domain)
            .await
            .map_err(|e| ApError::Other(format!("DBエラー: {}", e)))?
            .ok_or_else(|| ApError::Other("ローカルユーザーが見つかりません".to_string()));
    }

    resolve_fedi(state, t).await
}

async fn resolve_bsky(state: &AppState, actor_id_or_handle: &str) -> Result<Actor, ApError> {
    let bsky_resp = fetch_bsky_profile(&state.http_client, actor_id_or_handle)
        .await
        .map_err(|e| ApError::Other(format!("Bskyプロフィール取得失敗: {}", e)))?;
    let did = bsky_resp.did.clone();

    let now = chrono::Utc::now();
    let new_actor_id = generate_snowflake_id(now);
    let actor_id = state
        .actors
        .upsert_remote_bsky(
            new_actor_id,
            &did,
            &bsky_resp.handle,
            bsky_resp.display_name.as_deref(),
            bsky_resp.avatar.as_deref(),
            now,
        )
        .await
        .map_err(|e| ApError::Other(format!("DBエラー: {}", e)))?;

    state
        .actors
        .find_by_id(actor_id)
        .await
        .map_err(|e| ApError::Other(format!("DBエラー: {}", e)))?
        .ok_or_else(|| ApError::Other("アクター取得に失敗しました".to_string()))
}

async fn resolve_fedi(state: &AppState, target: &str) -> Result<Actor, ApError> {
    let target_uri = if target.starts_with("https://") || target.starts_with("http://") {
        target.to_string()
    } else {
        let parts: Vec<&str> = target.splitn(2, '@').collect();
        if parts.len() != 2 {
            return Err(ApError::Other(format!("ターゲット形式が不正です: {}", target)));
        }
        state.ap_client.resolve_webfinger(parts[0], parts[1]).await?
    };

    if let Some(existing) = state
        .actors
        .find_by_ap_uri(&target_uri)
        .await
        .map_err(|e| ApError::Other(format!("DBエラー: {}", e)))?
    {
        return Ok(existing);
    }

    let remote_ap = state.ap_client.fetch_actor(&target_uri).await?;
    let remote_inbox = remote_ap
        .inbox
        .clone()
        .ok_or_else(|| ApError::Other("リモートアクターにinboxがありません".to_string()))?;
    let remote_avatar_url = remote_ap.avatar_url();
    let remote_username = remote_ap
        .preferred_username
        .clone()
        .unwrap_or_else(|| target_uri.rsplit('/').next().unwrap_or("unknown").to_string());
    let remote_display_name = remote_ap.name.clone().unwrap_or_else(|| remote_username.clone());
    let remote_domain = target_uri.split('/').nth(2).unwrap_or("").to_string();
    let remote_bio = remote_ap
        .summary
        .as_deref()
        .map(seiran_common::jobs::inbound_activity_process::strip_html);
    let remote_emoji_map = remote_ap.emoji_map();
    let remote_profile_fields = remote_ap.profile_fields_json();

    let now = chrono::Utc::now();
    let new_actor_id = generate_snowflake_id(now);
    let remote_actor_id = state
        .actors
        .upsert_remote_fedi(
            new_actor_id,
            &target_uri,
            &remote_inbox,
            &remote_username,
            &remote_domain,
            &remote_display_name,
            remote_avatar_url.as_deref(),
            remote_bio.as_deref(),
            now,
            &remote_emoji_map,
            &remote_profile_fields,
        )
        .await
        .map_err(|e| ApError::Other(format!("DBエラー: {}", e)))?;

    state
        .actors
        .find_by_id(remote_actor_id)
        .await
        .map_err(|e| ApError::Other(format!("DBエラー: {}", e)))?
        .ok_or_else(|| ApError::Other("アクター取得に失敗しました".to_string()))
}

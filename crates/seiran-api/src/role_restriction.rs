//! user より制限されたロール（`silence`・`quarantine`）による投稿・働きかけの制限。
//!
//! - `silence`: パブリック投稿ができない。public を求められたらひかえめ（unlisted）へ黙って
//!   読み替える（Misskey のサイレンスと同じ。外部クライアントの投稿を失敗させないため）。
//! - `quarantine`: silence に加え、自分のフォロワー以外へ働きかけ（返信・引用・メンション・
//!   リアクション・DM）ができない。リポストは許す。
//!
//! 働きかけの相手が admin / moderator なら quarantine でも許す（調停者からの連絡に返信
//! できなくなるため）。投稿経路はネイティブ API（Misskey/Mastodon 互換 API もここを通る）と
//! ATP クライアントの `createRecord` 系の2つあり、両方から呼ぶ。

use seiran_common::mention::{
    extract_local_mention_actor_ids, extract_remote_mention_handles, RemoteMentionHandle,
};

use crate::error::ApiError;
use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleRestriction {
    None,
    Silence,
    Quarantine,
}

impl RoleRestriction {
    pub fn from_role(role: &str) -> Self {
        match role {
            "silence" => Self::Silence,
            "quarantine" => Self::Quarantine,
            _ => Self::None,
        }
    }

    /// 投稿の公開範囲を、このロールで許される最も広い範囲に収める。
    pub fn cap_visibility(self, visibility: &'static str) -> &'static str {
        if self != Self::None && visibility == "public" {
            "unlisted"
        } else {
            visibility
        }
    }

    /// フォロワー以外への働きかけを禁じるか。
    pub fn limits_interaction(self) -> bool {
        self == Self::Quarantine
    }
}

pub async fn restriction_of(state: &AppState, actor_id: i64) -> RoleRestriction {
    RoleRestriction::from_role(&crate::rate_limit::actor_role(state, actor_id).await)
}

/// quarantine のとき、`targets` の全員が「自分自身・自分のフォロワー・admin/moderator」の
/// いずれかであることを確かめる。
pub async fn require_can_interact(
    state: &AppState,
    actor_id: i64,
    restriction: RoleRestriction,
    targets: impl IntoIterator<Item = i64>,
) -> Result<(), ApiError> {
    if !restriction.limits_interaction() {
        return Ok(());
    }
    for target in targets {
        if target == actor_id {
            continue;
        }
        let follows_me = state
            .follows
            .find_status(target, actor_id)
            .await
            .map_err(|e| ApiError::Internal(format!("フォロー状態取得失敗: {}", e)))?
            .is_some_and(|s| s == "accepted");
        if follows_me {
            continue;
        }
        let role = crate::rate_limit::actor_role(state, target).await;
        if !matches!(role.as_str(), "admin" | "moderator") {
            return Err(ApiError::Forbidden("INTERACTION_RESTRICTED"));
        }
    }
    Ok(())
}

/// quarantine のとき、本文中のメンション先が全員働きかけてよい相手であることを確かめる。
/// リモート宛ての表記は自分のフォロワーの中から探す。見つからなければ（DB に無い相手も含め）
/// フォロワーではないので拒否する。
pub async fn require_mentions_allowed(
    state: &AppState,
    actor_id: i64,
    restriction: RoleRestriction,
    text: &str,
) -> Result<(), ApiError> {
    if !restriction.limits_interaction() {
        return Ok(());
    }
    let local_ids = extract_local_mention_actor_ids(text, &state.local_domain, &state.db).await;
    require_can_interact(state, actor_id, restriction, local_ids).await?;
    for handle in extract_remote_mention_handles(text, &state.local_domain) {
        let found = match &handle {
            RemoteMentionHandle::Fedi { username, domain } => {
                state
                    .follows
                    .has_follower_with_fedi_handle(actor_id, username, domain)
                    .await
            }
            RemoteMentionHandle::Atproto { handle } => {
                state
                    .follows
                    .has_follower_with_atproto_handle(actor_id, handle)
                    .await
            }
        }
        .map_err(|e| ApiError::Internal(format!("メンション先のフォロー状態取得失敗: {}", e)))?;
        if !found {
            return Err(ApiError::Forbidden("INTERACTION_RESTRICTED"));
        }
    }
    Ok(())
}

/// quarantine のとき、AT URI（`at://{did}/...`）や DID で指された相手が働きかけてよい相手か
/// 確かめる。DB に無い DID の相手はフォロワーではないので拒否する。
pub async fn require_can_interact_with_dids(
    state: &AppState,
    actor_id: i64,
    restriction: RoleRestriction,
    dids: &[&str],
) -> Result<(), ApiError> {
    if !restriction.limits_interaction() {
        return Ok(());
    }
    let mut targets = Vec::new();
    for did in dids.iter().copied() {
        let actor = state
            .actors
            .find_by_did(did)
            .await
            .map_err(|e| ApiError::Internal(format!("DID からのアクター取得失敗: {}", e)))?
            .ok_or(ApiError::Forbidden("INTERACTION_RESTRICTED"))?;
        targets.push(actor.id);
    }
    require_can_interact(state, actor_id, restriction, targets).await
}

/// `at://{did}/{collection}/{rkey}` から DID を取り出す。
pub fn did_of_at_uri(uri: &str) -> Option<&str> {
    uri.strip_prefix("at://")?
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{did_of_at_uri, RoleRestriction};

    #[test]
    fn cap_visibility_downgrades_public_only_for_restricted_roles() {
        assert_eq!(RoleRestriction::None.cap_visibility("public"), "public");
        assert_eq!(
            RoleRestriction::Silence.cap_visibility("public"),
            "unlisted"
        );
        assert_eq!(
            RoleRestriction::Quarantine.cap_visibility("public"),
            "unlisted"
        );
        assert_eq!(
            RoleRestriction::Silence.cap_visibility("followers_only"),
            "followers_only"
        );
        assert_eq!(
            RoleRestriction::Quarantine.cap_visibility("direct"),
            "direct"
        );
    }

    #[test]
    fn only_quarantine_limits_interaction() {
        assert!(!RoleRestriction::from_role("user").limits_interaction());
        assert!(!RoleRestriction::from_role("silence").limits_interaction());
        assert!(RoleRestriction::from_role("quarantine").limits_interaction());
    }

    #[test]
    fn did_of_at_uri_extracts_authority() {
        assert_eq!(
            did_of_at_uri("at://did:plc:abc/app.bsky.feed.post/3k"),
            Some("did:plc:abc")
        );
        assert_eq!(did_of_at_uri("https://example.com"), None);
        assert_eq!(did_of_at_uri("at:///x"), None);
    }
}

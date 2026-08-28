//! フォロー対象文字列（ローカルユーザー名 / `@user@domain` / `https://...` / `did:...` /
//! ATPハンドル）の種別判定ロジック。
//!
//! `seiran-api` の `handlers::follows::create_follow` と `handlers::target_resolve::
//! resolve_and_upsert_target` で分岐条件が重複していたため、判定順序・条件を変えずに
//! ここへ統合した（挙動不変のリファクタ）。

use crate::username::strip_local_domain_suffix;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowTargetKind {
    /// ローカルユーザー名（ドメイン無し）
    Local(String),
    /// Bsky ATP（DID またはハンドル）
    Bsky(String),
    /// Fedi（AP actor URI または `user@domain` acct）
    Fedi(String),
}

/// `target` を trim・先頭 `@` 除去したうえで種別判定する。
pub fn classify_follow_target(target: &str, local_domain: &str) -> FollowTargetKind {
    let t = target.trim().trim_start_matches('@');

    // HTTP(S) URI → Fedi AP フォロー（ATP ハンドル判定より先に弾く）
    if t.starts_with("https://") || t.starts_with("http://") {
        return FollowTargetKind::Fedi(t.to_string());
    }

    // DID 形式 → Bsky ATP フォロー
    if t.starts_with("did:") {
        return FollowTargetKind::Bsky(t.to_string());
    }

    // ローカルユーザーの完全な ATP ハンドル表記（`user.{local_domain}`）→ AppView へ問い合わせず
    // ローカルフォローとして処理する。判定せず Bsky 経路に流すと、AppView 解決結果（ハンドル
    // 表記そのもの）で `upsert_remote_bsky` の `ON CONFLICT (at_did)` が発火し、ローカル
    // アクターの `username` 列を壊す（実際に発生した事故、`docs/protocols.md` 4節参照）。
    if let Some(username) = strip_local_domain_suffix(t, local_domain) {
        return FollowTargetKind::Local(username.to_string());
    }

    // ATP ハンドル（ドット含み・@なし・http なし）→ Bsky ATP フォロー
    if t.contains('.') && !t.contains('@') {
        return FollowTargetKind::Bsky(t.to_string());
    }

    // ローカルユーザー名（@ なし・ドットなし）→ ローカルフォロー
    let parts: Vec<&str> = t.splitn(2, '@').collect();
    if parts.len() == 1 {
        return FollowTargetKind::Local(parts[0].to_string());
    }
    // `alice@seiran.org` → ローカルフォロー
    if parts.len() == 2 && parts[1] == local_domain {
        return FollowTargetKind::Local(parts[0].to_string());
    }

    // Fedi リモート (`alice@mastodon.social`)
    FollowTargetKind::Fedi(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL_DOMAIN: &str = "seiran.example";

    #[test]
    fn https_uri_is_fedi() {
        assert_eq!(
            classify_follow_target("https://mastodon.social/users/alice", LOCAL_DOMAIN),
            FollowTargetKind::Fedi("https://mastodon.social/users/alice".to_string())
        );
    }

    #[test]
    fn http_uri_is_fedi() {
        assert_eq!(
            classify_follow_target("http://mastodon.social/users/alice", LOCAL_DOMAIN),
            FollowTargetKind::Fedi("http://mastodon.social/users/alice".to_string())
        );
    }

    #[test]
    fn did_is_bsky() {
        assert_eq!(
            classify_follow_target("did:plc:abc123", LOCAL_DOMAIN),
            FollowTargetKind::Bsky("did:plc:abc123".to_string())
        );
    }

    #[test]
    fn local_atp_handle_is_local() {
        assert_eq!(
            classify_follow_target("alice.seiran.example", LOCAL_DOMAIN),
            FollowTargetKind::Local("alice".to_string())
        );
    }

    #[test]
    fn remote_atp_handle_is_bsky() {
        assert_eq!(
            classify_follow_target("alice.bsky.social", LOCAL_DOMAIN),
            FollowTargetKind::Bsky("alice.bsky.social".to_string())
        );
    }

    #[test]
    fn bare_username_is_local() {
        assert_eq!(
            classify_follow_target("alice", LOCAL_DOMAIN),
            FollowTargetKind::Local("alice".to_string())
        );
    }

    #[test]
    fn leading_at_is_stripped() {
        assert_eq!(
            classify_follow_target("@alice", LOCAL_DOMAIN),
            FollowTargetKind::Local("alice".to_string())
        );
    }

    #[test]
    fn local_acct_is_local() {
        assert_eq!(
            classify_follow_target("alice@seiran.example", LOCAL_DOMAIN),
            FollowTargetKind::Local("alice".to_string())
        );
    }

    #[test]
    fn remote_acct_is_fedi() {
        assert_eq!(
            classify_follow_target("alice@mastodon.social", LOCAL_DOMAIN),
            FollowTargetKind::Fedi("alice@mastodon.social".to_string())
        );
    }

    #[test]
    fn whitespace_is_trimmed() {
        assert_eq!(
            classify_follow_target("  alice  ", LOCAL_DOMAIN),
            FollowTargetKind::Local("alice".to_string())
        );
    }
}

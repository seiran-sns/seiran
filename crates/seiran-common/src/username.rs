//! ローカルユーザー名の命名規則。
//!
//! ローカルユーザー名は「ドメイン名の1ラベルとして成立する文字列」でなければならない
//! （英数字とハイフンのみ、先頭/末尾はハイフン不可、ピリオド不可）。理由は2つ:
//!
//! 1. ATPハンドルは `{username}.{domain}` の形で組み立てる（`handlers/auth.rs` の
//!    `register()` 等）。username 自体が DNS ラベルとして妥当でなければ不正なハンドルになる。
//! 2. `@` で始まり途中に `@` を含まない文字列（ローカルID `@user` か ATP ハンドル
//!    `@user.bsky.social` か）を見たとき、`.` を含むかどうかでどちらかを判別できる
//!    （ローカルユーザー名に `.` が現れないことが前提のため）。
//!
//! 大文字小文字: username 自体には大文字を許可する（表示上の見た目を尊重する）が、
//! ATPハンドルを組み立てる箇所（DID document の `alsoKnownAs`、resolveHandle/well-known
//! の応答等）では必ず小文字化した値を使う。DNS/HTTPホスト名は経路上（プロキシ・リゾルバ・
//! Bluesky側の正規化）で小文字化されうるため、大文字混じりのハンドルを PLC に登録すると
//! 恒久的に解決不能（bsky.app 上で `handle.invalid`）になる実障害が過去に発生した。
//! ハンドルの大小差だけで別ユーザーになれると衝突するため、ユーザー名の重複検証は
//! 大文字小文字を区別しない（`ActorRepository::find_by_username_domain` 等）。

/// list-relay 仮想アクター（リスト機能 #63 のプロキシフォロー用）の予約ユーザー名。
pub const PROXY_ACTOR_USERNAME: &str = "list-relay";

/// relay-agent 仮想アクター（Fediverseリレー参加機能 #140 のFollow/Undo送信用）の予約ユーザー名。
pub const RELAY_AGENT_USERNAME: &str = "relay-agent";

/// 一般ユーザーが登録できない予約ユーザー名（`register()` で明示的に拒否する）。
pub const RESERVED_LOCAL_USERNAMES: &[&str] = &[PROXY_ACTOR_USERNAME, RELAY_AGENT_USERNAME];

/// ユーザー名がDNSラベルとして妥当か（英数字・ハイフンのみ、先頭/末尾はハイフン不可、1〜63文字）。
pub fn is_valid_local_username(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return false;
    }
    let is_alnum = |b: u8| b.is_ascii_alphanumeric();
    if !is_alnum(bytes[0]) || !is_alnum(bytes[bytes.len() - 1]) {
        return false;
    }
    bytes.iter().all(|&b| is_alnum(b) || b == b'-')
}

/// ATPハンドル用に正規化したユーザー名（小文字化のみ。文字種の妥当性は
/// `is_valid_local_username` で既に検証済みであることが前提）。
pub fn to_atp_username(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// 予約ユーザー名か（大文字小文字を区別しない）。
pub fn is_reserved_username(s: &str) -> bool {
    RESERVED_LOCAL_USERNAMES
        .iter()
        .any(|r| r.eq_ignore_ascii_case(s))
}

/// `{username}.{local_domain}` 形式（ローカルユーザーのATPハンドル表記）から
/// username部分を取り出す。`local_domain` サフィックスでないか、その手前が空なら `None`。
/// 大文字小文字は区別しない（`ActorRepository::find_by_username_domain` と同じ扱い）。
///
/// Bskyリモートアクター解決系（`resolve_bsky`/`follow_bsky` 等）は、この形式の文字列を
/// AppViewへ問い合わせる前にここでローカル判定できる。判定を怠ると、AppView解決結果を
/// `at_did` の `ON CONFLICT` でupsertする際にローカルアクターの `username` 列を
/// このハンドル表記自体で上書きしてしまう事故につながる（過去に実際発生）。
pub fn strip_local_domain_suffix<'a>(s: &'a str, local_domain: &str) -> Option<&'a str> {
    let suffix_len = local_domain.len();
    if s.len() <= suffix_len + 1 || !s.is_char_boundary(s.len() - suffix_len) {
        return None;
    }
    let (prefix, domain_part) = s.split_at(s.len() - suffix_len);
    if !domain_part.eq_ignore_ascii_case(local_domain) {
        return None;
    }
    prefix.strip_suffix('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_alnum_and_hyphen() {
        assert!(is_valid_local_username("alice"));
        assert!(is_valid_local_username("alice123"));
        assert!(is_valid_local_username("list-relay"));
    }

    #[test]
    fn rejects_period_and_underscore() {
        assert!(!is_valid_local_username("list.relay"));
        assert!(!is_valid_local_username("list_relay"));
    }

    #[test]
    fn rejects_leading_trailing_hyphen() {
        assert!(!is_valid_local_username("-alice"));
        assert!(!is_valid_local_username("alice-"));
    }

    #[test]
    fn accepts_uppercase_but_lowercases_for_atp() {
        assert!(is_valid_local_username("Alice"));
        assert_eq!(to_atp_username("Alice"), "alice");
        assert_eq!(to_atp_username("ALICE"), "alice");
    }

    #[test]
    fn rejects_empty_and_too_long() {
        assert!(!is_valid_local_username(""));
        assert!(!is_valid_local_username(&"a".repeat(64)));
        assert!(is_valid_local_username(&"a".repeat(63)));
    }

    #[test]
    fn reserved_is_case_insensitive() {
        assert!(is_reserved_username("list-relay"));
        assert!(is_reserved_username("List-Relay"));
        assert!(is_reserved_username("relay-agent"));
        assert!(!is_reserved_username("alice"));
    }

    #[test]
    fn strip_local_domain_suffix_extracts_username() {
        assert_eq!(
            strip_local_domain_suffix("alice.seiran-beta.org", "seiran-beta.org"),
            Some("alice")
        );
        assert_eq!(
            strip_local_domain_suffix("Alice.Seiran-Beta.org", "seiran-beta.org"),
            Some("Alice")
        );
    }

    #[test]
    fn strip_local_domain_suffix_rejects_other_domain() {
        assert_eq!(
            strip_local_domain_suffix("alice.bsky.social", "seiran-beta.org"),
            None
        );
        assert_eq!(
            strip_local_domain_suffix("alice.other-seiran-beta.org", "seiran-beta.org"),
            None
        );
    }

    #[test]
    fn strip_local_domain_suffix_rejects_bare_domain_and_short_input() {
        assert_eq!(
            strip_local_domain_suffix("seiran-beta.org", "seiran-beta.org"),
            None
        );
        assert_eq!(strip_local_domain_suffix(".seiran-beta.org", "seiran-beta.org"), None);
        assert_eq!(strip_local_domain_suffix("org", "seiran-beta.org"), None);
    }
}

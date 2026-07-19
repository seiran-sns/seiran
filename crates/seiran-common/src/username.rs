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

/// list-relay 仮想アクター（リスト機能 #63 のプロキシフォロー用）の予約ユーザー名。
pub const PROXY_ACTOR_USERNAME: &str = "list-relay";

/// 一般ユーザーが登録できない予約ユーザー名（`register()` で明示的に拒否する）。
///
/// `vite`/`react-refresh` はプロフィールページ `GET /@:handle` との名前衝突を防ぐため
/// 予約している。フロントエンド開発サーバー（Vite）は `/@vite/client`・`/@react-refresh`
/// を自身の内部モジュールとして特別扱いするため、`frontend/vite.config.ts` の開発用プロキシは
/// これらを除外してバックエンドへ転送しない（実機確認: 除外前は `/@vite/client` がバックエンドの
/// `/@:handle` ルートに奪われ、Viteクライアントが読み込めず白画面になった）。この結果、万一
/// 同名のローカルユーザーが存在すると、そのプロフィールページだけ開発サーバー経由では
/// OGP注入が効かなくなる（本番の nginx 配信では発生しない開発環境限定の制約）ため、
/// 登録自体を禁止して矛盾を作らないようにする。
pub const RESERVED_LOCAL_USERNAMES: &[&str] = &[PROXY_ACTOR_USERNAME, "vite", "react-refresh"];

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

/// 予約ユーザー名か（大文字小文字を区別しない）。
pub fn is_reserved_username(s: &str) -> bool {
    RESERVED_LOCAL_USERNAMES.iter().any(|r| r.eq_ignore_ascii_case(s))
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
    fn rejects_empty_and_too_long() {
        assert!(!is_valid_local_username(""));
        assert!(!is_valid_local_username(&"a".repeat(64)));
        assert!(is_valid_local_username(&"a".repeat(63)));
    }

    #[test]
    fn reserved_is_case_insensitive() {
        assert!(is_reserved_username("list-relay"));
        assert!(is_reserved_username("List-Relay"));
        assert!(!is_reserved_username("alice"));
    }

    #[test]
    fn reserved_vite_internal_names() {
        assert!(is_reserved_username("vite"));
        assert!(is_reserved_username("react-refresh"));
    }
}

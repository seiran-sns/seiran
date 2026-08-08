//! 自ホストドメインの実行時表現。
//!
//! ドメインは `instance_domain` テーブルに一度だけ確定される不変値（`repository::instance_domain`
//! 参照）。プロセス起動時に一度だけDBを読み、確定済みなら以後読み取り専用（ロックフリー）で
//! 全ハンドラ・全ロール（api/federation/worker）から参照される。
//!
//! 後方互換パス（`.env`の`LOCAL_DOMAIN`があればそのままDBへ移行する）に加え、初回セットアップ
//! （`handlers::setup`）でHostヘッダーから確定するフローも持つ（`domain_candidate_from_host`）。

use std::net::IpAddr;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};

use crate::repository::{ConfirmOutcome, InstanceDomainRepository};

/// 自ホストドメイン。`Arc<OnceLock<String>>` により「一度きりの不可逆な遷移」を表現する。
/// `Clone`すると`Arc`が共有されるため、確定操作は同一プロセス内の全ての参照箇所に
/// 即座に伝播する。
#[derive(Clone)]
pub struct LocalDomain(Arc<OnceLock<String>>);

impl LocalDomain {
    /// 未確定の状態で生成する。
    pub fn unresolved() -> Self {
        Self(Arc::new(OnceLock::new()))
    }

    /// 確定済みならその値、未確定なら`"localhost"`を返す。
    pub fn as_str(&self) -> &str {
        self.0.get().map(String::as_str).unwrap_or("localhost")
    }

    pub fn is_confirmed(&self) -> bool {
        self.0.get().is_some()
    }

    /// 確定させる。既に確定済みの場合は無視する（`OnceLock`は先勝ち）。
    pub fn set_confirmed(&self, domain: String) {
        let _ = self.0.set(domain);
    }
}

impl std::ops::Deref for LocalDomain {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for LocalDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::fmt::Debug for LocalDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("LocalDomain").field(&self.as_str()).finish()
    }
}

impl PartialEq<str> for LocalDomain {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}
impl PartialEq<LocalDomain> for str {
    fn eq(&self, other: &LocalDomain) -> bool {
        self == other.as_str()
    }
}
impl PartialEq<&str> for LocalDomain {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}
impl PartialEq<LocalDomain> for &str {
    fn eq(&self, other: &LocalDomain) -> bool {
        *self == other.as_str()
    }
}
impl PartialEq<String> for LocalDomain {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}
impl PartialEq<LocalDomain> for String {
    fn eq(&self, other: &LocalDomain) -> bool {
        self.as_str() == other.as_str()
    }
}

/// プロセス起動時に一度だけ呼ぶ。`instance_domain`テーブルが既に確定済みならその値を使う。
/// 未確定なら、`legacy_env_domain`（`.env`の`LOCAL_DOMAIN`、既存インストールとの後方互換用）が
/// あればそれで確定させる。どちらも無ければ未確定のまま返す
/// （`as_str()`は`"localhost"`を返し続ける。Hostヘッダーからの確定フローは次フェーズで追加）。
pub async fn resolve_local_domain(
    repo: &dyn InstanceDomainRepository,
    legacy_env_domain: Option<String>,
) -> LocalDomain {
    let domain = LocalDomain::unresolved();

    match repo.get().await {
        Ok(Some(confirmed)) => {
            domain.set_confirmed(confirmed);
        }
        Ok(None) => {
            if let Some(env_domain) = legacy_env_domain {
                match repo.confirm(&env_domain).await {
                    Ok(ConfirmOutcome::Confirmed(d)) => {
                        tracing::info!(
                            "[local_domain] .envのLOCAL_DOMAIN（{}）をinstance_domainへ移行し確定しました",
                            d
                        );
                        domain.set_confirmed(d);
                    }
                    Ok(ConfirmOutcome::AlreadyConfirmed(d)) => {
                        domain.set_confirmed(d);
                    }
                    Err(e) => {
                        tracing::error!("[local_domain] 後方互換移行に失敗しました: {}", e);
                    }
                }
            }
        }
        Err(e) => {
            tracing::error!("[local_domain] instance_domain取得に失敗しました: {}", e);
        }
    }

    domain
}

/// HTTPリクエストの`Host`ヘッダー値からDBに確定してよいドメイン候補を判定する
/// （純粋関数、HTTP層に依存しない）。連合には固定ドメインが必須なため、以下は
/// 確定候補として扱わない（`None`を返す）:
/// - ヘッダーが空
/// - `localhost`
/// - IPアドレス直打ち（IPv4/IPv6、`[::1]:port`形式含む）
///
/// ポート番号（`:port`、IPv6の`[::1]:port`形式含む）は除去し、小文字化・末尾ドット除去のみ
/// 行う。それ以外の文字列はそのままドメイン候補として採用する。
pub fn domain_candidate_from_host(host: &str) -> Option<String> {
    let host = host.trim();
    if host.is_empty() {
        return None;
    }

    let without_port = strip_port(host);
    let normalized = without_port.trim_end_matches('.').to_ascii_lowercase();

    if normalized.is_empty() || normalized == "localhost" {
        return None;
    }
    if IpAddr::from_str(&normalized).is_ok() {
        return None;
    }

    Some(normalized)
}

/// `Host`ヘッダーからポート番号を除去する。IPv6の`[::1]:8080`形式では
/// 角括弧内（`::1`）を返す。角括弧なしでコロンが2個以上の場合はIPv6アドレス本体と
/// みなしそのまま返す（本来`[...]`必須だが、角括弧なしの生IPv6を誤ってポート分離
/// しないための保険）。それ以外は最後の`:`より前を返す。
fn strip_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return &rest[..end];
        }
        return host;
    }
    if host.matches(':').count() >= 2 {
        return host;
    }
    match host.rfind(':') {
        Some(idx) => &host[..idx],
        None => host,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_domain() {
        assert_eq!(
            domain_candidate_from_host("seiran-beta.org"),
            Some("seiran-beta.org".to_string())
        );
    }

    #[test]
    fn strips_port_and_lowercases() {
        assert_eq!(
            domain_candidate_from_host("Seiran-Beta.org:3000"),
            Some("seiran-beta.org".to_string())
        );
    }

    #[test]
    fn strips_trailing_dot() {
        assert_eq!(
            domain_candidate_from_host("seiran-beta.org."),
            Some("seiran-beta.org".to_string())
        );
    }

    #[test]
    fn rejects_localhost() {
        assert_eq!(domain_candidate_from_host("localhost"), None);
        assert_eq!(domain_candidate_from_host("localhost:3000"), None);
        assert_eq!(domain_candidate_from_host("LOCALHOST"), None);
    }

    #[test]
    fn rejects_ipv4() {
        assert_eq!(domain_candidate_from_host("127.0.0.1"), None);
        assert_eq!(domain_candidate_from_host("192.168.1.1:8080"), None);
    }

    #[test]
    fn rejects_ipv6() {
        assert_eq!(domain_candidate_from_host("::1"), None);
        assert_eq!(domain_candidate_from_host("[::1]"), None);
        assert_eq!(domain_candidate_from_host("[::1]:8080"), None);
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(domain_candidate_from_host(""), None);
        assert_eq!(domain_candidate_from_host("   "), None);
    }
}

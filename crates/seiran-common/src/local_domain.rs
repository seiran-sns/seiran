//! 自ホストドメインの実行時表現。
//!
//! ドメインは `instance_domain` テーブルに一度だけ確定される不変値（`repository::instance_domain`
//! 参照）。プロセス起動時に一度だけDBを読み、確定済みなら以後読み取り専用（ロックフリー）で
//! 全ハンドラ・全ロール（api/federation/worker）から参照される。
//!
//! 現状（フェーズ1）は「`.env`の`LOCAL_DOMAIN`があればそれをそのままDBへ移行する」後方互換
//! パスのみを持つ。Hostヘッダーからの確定フロー・シングルホストモードは次フェーズで追加する。

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

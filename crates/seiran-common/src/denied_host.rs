//! 拒否サーバー（管理画面「サーバーブロック」の「拒否サーバー」）の判定。
//!
//! 拒否サーバーのアクターについては、次のように扱う（`docs/architecture.md`「拒否サーバー」）。
//! - 投稿: ローカルの誰かがフォローしているか、リストのメンバーでなければ受け取らない。
//! - 働きかけ: 宛先のローカルユーザー本人がフォローしていなければ受け取らない。
//! - グローバル・ハッシュタグタイムラインには出さない。
//!
//! 受信のたびに DB を引かないよう、ホスト一覧はプロセス内に短時間キャッシュする。
//! 複数プロセス構成では、保存したプロセス以外への反映は`CACHE_TTL`だけ遅れる。

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use sqlx::PgPool;

const CACHE_TTL: Duration = Duration::from_secs(30);

struct Cache {
    loaded_at: Instant,
    hosts: Arc<Vec<String>>,
}

static CACHE: RwLock<Option<Cache>> = RwLock::new(None);

/// 管理画面で保存した直後に、このプロセスのキャッシュを捨てる。
pub fn invalidate_cache() {
    if let Ok(mut c) = CACHE.write() {
        *c = None;
    }
}

async fn hosts(pool: &PgPool) -> Arc<Vec<String>> {
    if let Ok(c) = CACHE.read() {
        if let Some(c) = c.as_ref() {
            if c.loaded_at.elapsed() < CACHE_TTL {
                return c.hosts.clone();
            }
        }
    }
    match crate::repository::denied_host::list_hosts(pool).await {
        Ok(list) => {
            let hosts = Arc::new(list);
            if let Ok(mut c) = CACHE.write() {
                *c = Some(Cache {
                    loaded_at: Instant::now(),
                    hosts: hosts.clone(),
                });
            }
            hosts
        }
        Err(e) => {
            // 読めないときは直前の一覧で判定を続ける（無ければ拒否なし）。受信全体を
            // 止めるよりは、一時的に拒否が効かないほうが害が小さい。
            tracing::warn!("[DeniedHost] 拒否ホスト一覧の取得に失敗: {}", e);
            CACHE
                .read()
                .ok()
                .and_then(|c| c.as_ref().map(|c| c.hosts.clone()))
                .unwrap_or_default()
        }
    }
}

/// `host`が`denied`のいずれか、またはそのサブドメインか（SQL の`host_is_denied`と同じ規則）。
pub fn host_matches(host: &str, denied: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    denied.iter().any(|d| {
        host == *d
            || (host.len() > d.len()
                && host.ends_with(d.as_str())
                && host.as_bytes()[host.len() - d.len() - 1] == b'.')
    })
}

/// URL のホスト部分（小文字）。
pub fn url_host(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .host_str()
        .map(|h| h.to_ascii_lowercase())
}

/// ホストが拒否サーバーか。
pub async fn is_denied_host(pool: &PgPool, host: &str) -> bool {
    let denied = hosts(pool).await;
    !denied.is_empty() && host_matches(host, &denied)
}

/// URL（アクター URI・オブジェクト URI・PDS エンドポイント）のホストが拒否サーバーか。
pub async fn is_denied_url(pool: &PgPool, url: &str) -> bool {
    let denied = hosts(pool).await;
    if denied.is_empty() {
        return false;
    }
    url_host(url).is_some_and(|h| host_matches(&h, &denied))
}

/// 拒否ホストが1件でも登録されているか（未登録なら関係の問い合わせを省くため）。
pub async fn any_denied(pool: &PgPool) -> bool {
    !hosts(pool).await.is_empty()
}

/// PDS ホストを解決し直す間隔。PDS の引っ越しはまれなので1日で十分。
const PDS_HOST_TTL: chrono::Duration = chrono::Duration::hours(24);

/// DID ドキュメントの`#atproto_pds`のホスト。
async fn resolve_pds_host(did: &str) -> Option<String> {
    let doc = crate::atp::did_resolve::fetch_raw_did_document(did)
        .await
        .ok()?;
    let qualified = format!("{}#atproto_pds", did);
    let endpoint = doc
        .get("service")?
        .as_array()?
        .iter()
        .find(|s| {
            let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("");
            id == "#atproto_pds" || id == qualified
        })?
        .get("serviceEndpoint")?
        .as_str()?
        .to_string();
    url_host(&endpoint)
}

/// ATP アクターの`pds_host`が未解決か古ければ解決し直す。
pub async fn refresh_pds_host(pool: &PgPool, actor_id: i64, did: &str) {
    let stale_before = chrono::Utc::now() - PDS_HOST_TTL;
    match crate::repository::denied_host::pds_host_needs_refresh(pool, actor_id, stale_before).await
    {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            tracing::warn!(
                "[DeniedHost] pds_host 確認失敗 actor_id={}: {}",
                actor_id,
                e
            );
            return;
        }
    }
    let host = resolve_pds_host(did).await;
    if let Err(e) =
        crate::repository::denied_host::set_pds_host(pool, actor_id, host.as_deref()).await
    {
        tracing::warn!(
            "[DeniedHost] pds_host 保存失敗 actor_id={}: {}",
            actor_id,
            e
        );
    }
}

/// ATP 経由で届いたアクターが拒否サーバーに属するか。拒否ホストが1件も無ければ PDS ホストの
/// 解決もしない。
pub async fn atp_actor_is_denied(pool: &PgPool, actor_id: i64, did: &str) -> bool {
    if !any_denied(pool).await {
        return false;
    }
    refresh_pds_host(pool, actor_id, did).await;
    crate::repository::denied_host::actor_is_denied(pool, actor_id)
        .await
        .unwrap_or(false)
}

/// ローカルからフォローされているかリストに入っている ATP アクターのうち、PDS ホストが
/// 未解決のものを解決する。拒否ホストを保存したときに、既存の投稿もグローバル・
/// ハッシュタグタイムラインから外れるようにするため。
pub async fn backfill_pds_hosts(pool: &PgPool) {
    loop {
        let batch =
            match crate::repository::denied_host::atp_actors_without_pds_host(pool, 100).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("[DeniedHost] pds_host バックフィル対象の取得に失敗: {}", e);
                    return;
                }
            };
        if batch.is_empty() {
            return;
        }
        for (actor_id, did) in batch {
            let host = resolve_pds_host(&did).await;
            // 解決できなくても確認時刻は進めるので、同じ行を取り続けることはない。
            if let Err(e) =
                crate::repository::denied_host::set_pds_host(pool, actor_id, host.as_deref()).await
            {
                tracing::warn!(
                    "[DeniedHost] pds_host 保存失敗 actor_id={}: {}",
                    actor_id,
                    e
                );
                return;
            }
        }
    }
}

/// 管理画面の textarea の1行を拒否ホストに正規化する。空行・コメント（`#`始まり）は`None`。
/// `https://`・パス・ポート・`*.`・`@`前置き・末尾の`.`を取り除き、小文字にする。
pub fn normalize_host_line(line: &str) -> Option<String> {
    let mut s = line.trim();
    if s.is_empty() || s.starts_with('#') {
        return None;
    }
    if let Some(i) = s.find("://") {
        s = &s[i + 3..];
    }
    let s = s.split(['/', '?', '#']).next().unwrap_or("");
    let s = s.rsplit('@').next().unwrap_or("");
    let s = s.split(':').next().unwrap_or("");
    let s = s.trim_start_matches("*.").trim_start_matches('.');
    let s = s.trim_end_matches('.').to_ascii_lowercase();
    if s.is_empty() || s.chars().any(|c| c.is_whitespace() || c == '*') {
        return None;
    }
    Some(s)
}

/// textarea 全体を正規化し、重複を除いてホスト名順に並べる。
pub fn normalize_host_list(text: &str) -> Vec<String> {
    let mut hosts: Vec<String> = text.lines().filter_map(normalize_host_line).collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_exact_and_subdomains_only() {
        let denied = vec!["spam.example".to_string()];
        assert!(host_matches("spam.example", &denied));
        assert!(host_matches("a.b.spam.example", &denied));
        assert!(host_matches("SPAM.example.", &denied));
        assert!(!host_matches("notspam.example", &denied));
        assert!(!host_matches("example", &denied));
        assert!(!host_matches("", &denied));
    }

    #[test]
    fn normalizes_textarea_lines() {
        assert_eq!(
            normalize_host_list(
                "https://Spam.Example/users/a\n\n# comment\n*.foo.test\n@user@bar.test\nbaz.test:8443\nspam.example\n"
            ),
            vec!["bar.test", "baz.test", "foo.test", "spam.example"]
        );
        assert_eq!(normalize_host_line("   "), None);
        assert_eq!(normalize_host_line("bad host"), None);
    }
}

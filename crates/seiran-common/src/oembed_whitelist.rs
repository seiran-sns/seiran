//! oEmbed embed機能の許可ドメイン（`site_settings.oembed_allowed_domains`）を
//! TTLキャッシュ経由で判定する。複数プロセス（split-role構成のapi/worker）で
//! それぞれ独立にキャッシュを持ち、管理画面の変更は最大TTL秒後に反映される
//! （即時反映は要件外。Redis不要、RwLock + Instantのプロセス内キャッシュのみ）。
//!
//! 設定値は改行区切りで1行1エントリ、各行は`domain`または`domain,oembed_endpoint`の
//! いずれか。後者はHTMLページに`<link rel="alternate" type=".../oembed">`のdiscovery
//! タグを載せていないが、oEmbedエンドポイント自体は提供しているサイト（例:
//! Vimeo）向けの救済で、指定した場合はHTML内のdiscoveryタグ探索をスキップし、
//! 常にこのエンドポイントを`?url=<対象URL>&format=json`付きで直接叩く。

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

use crate::repository::SiteSettingsRepository;

const SETTINGS_KEY: &str = "oembed_allowed_domains";
/// 管理画面の変更が反映されるまでの最大遅延。embed許可ドメインの変更頻度は
/// 極めて低いため、厳密な即時反映より頻繁なDBアクセスを避けることを優先する。
const TTL: Duration = Duration::from_secs(60);

/// ホワイトリスト1行分。`endpoint`はそのドメインのURLについて、HTML discoveryを
/// 試みず常にこのoEmbedエンドポイントを直接使う場合のみ`Some`。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    domain: String,
    endpoint: Option<String>,
}

pub struct OembedWhitelist {
    site_settings: Arc<dyn SiteSettingsRepository>,
    // (正規化済みエントリ一覧, 最終取得時刻)。初期値は「TTL経過済み」として、
    // 初回呼び出しで必ず1回はDBを引くようにする。
    cached: RwLock<(Vec<Entry>, Instant)>,
}

impl OembedWhitelist {
    pub fn new(site_settings: Arc<dyn SiteSettingsRepository>) -> Self {
        Self {
            site_settings,
            cached: RwLock::new((Vec::new(), Instant::now() - TTL - Duration::from_secs(1))),
        }
    }

    /// `embed_src`のホストが許可ドメイン、またはそのサブドメインかどうか。
    /// `host == entry || host.ends_with(".{entry}")`で判定する（文字列部分一致ではなく
    /// ホスト名の厳密比較。`evil.com/?x=youtube.com`のような細工を防ぐため、
    /// 呼び出し元は生の文字列ではなく`Url::parse(embed_src)?.host_str()`を渡すこと）。
    pub async fn is_allowed(&self, host: &str) -> bool {
        let host = host.to_lowercase();
        let entries = self.entries().await;
        entries
            .iter()
            .any(|e| host == e.domain || host.ends_with(&format!(".{}", e.domain)))
    }

    /// `embed_src`をホワイトリスト判定した上で返す。`embed_src`が`None`、URLとして
    /// パースできない、ホストが取れない、ホワイトリスト外のいずれかであれば`None`。
    /// `OgpFetch`/`LinkCardEmbedResolve`ジョブ・Bsky embed選択の3箇所で共有する。
    pub async fn filter_embed_src(&self, embed_src: Option<&str>) -> Option<String> {
        let embed_src = embed_src?;
        let host = url::Url::parse(embed_src)
            .ok()?
            .host_str()
            .map(str::to_string)?;
        if self.is_allowed(&host).await {
            Some(embed_src.to_string())
        } else {
            None
        }
    }

    /// 対象URLのホストにマッチするエントリに固定oEmbedエンドポイントが設定されていれば
    /// それを返す（HTML discoveryをスキップして直接叩くために使う）。マッチするエントリが
    /// 無い、またはエンドポイント未設定なら`None`（通常のHTML discoveryにフォールバック）。
    pub async fn fixed_endpoint_for(&self, url: &str) -> Option<String> {
        let host = url::Url::parse(url).ok()?.host_str()?.to_lowercase();
        let entries = self.entries().await;
        entries
            .iter()
            .find(|e| host == e.domain || host.ends_with(&format!(".{}", e.domain)))
            .and_then(|e| e.endpoint.clone())
    }

    async fn entries(&self) -> Vec<Entry> {
        {
            let guard = self.cached.read().await;
            if guard.1.elapsed() < TTL {
                return guard.0.clone();
            }
        }
        let mut guard = self.cached.write().await;
        // 書き込みロック取得までの間に他タスクが更新済みかもしれないので二重チェック。
        if guard.1.elapsed() < TTL {
            return guard.0.clone();
        }
        let raw = self
            .site_settings
            .get(SETTINGS_KEY)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        let entries: Vec<Entry> = raw
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .filter_map(|l| {
                let mut parts = l.splitn(2, ',');
                let domain = parts.next()?.trim().to_lowercase();
                if domain.is_empty() {
                    return None;
                }
                let endpoint = parts
                    .next()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                Some(Entry { domain, endpoint })
            })
            .collect();
        *guard = (entries.clone(), Instant::now());
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct FakeSiteSettings {
        value: Mutex<Option<String>>,
        get_calls: Mutex<u32>,
    }

    #[async_trait]
    impl SiteSettingsRepository for FakeSiteSettings {
        async fn get(&self, _key: &str) -> Result<Option<String>, sqlx::Error> {
            *self.get_calls.lock().unwrap() += 1;
            Ok(self.value.lock().unwrap().clone())
        }
        async fn set(&self, _key: &str, _value: &str) -> Result<(), sqlx::Error> {
            unreachable!()
        }
        async fn get_all(&self) -> Result<HashMap<String, String>, sqlx::Error> {
            unreachable!()
        }
    }

    fn whitelist(value: &str) -> OembedWhitelist {
        OembedWhitelist::new(Arc::new(FakeSiteSettings {
            value: Mutex::new(Some(value.to_string())),
            get_calls: Mutex::new(0),
        }))
    }

    #[tokio::test]
    async fn matches_exact_and_subdomain() {
        let wl = whitelist("youtube.com\nopen.spotify.com");
        assert!(wl.is_allowed("youtube.com").await);
        assert!(wl.is_allowed("www.youtube.com").await);
        assert!(wl.is_allowed("open.spotify.com").await);
        assert!(!wl.is_allowed("evil-youtube.com").await);
        assert!(!wl.is_allowed("youtube.com.evil.com").await);
    }

    #[tokio::test]
    async fn caches_within_ttl() {
        let repo = Arc::new(FakeSiteSettings {
            value: Mutex::new(Some("youtube.com".to_string())),
            get_calls: Mutex::new(0),
        });
        let wl = OembedWhitelist::new(repo.clone());
        assert!(wl.is_allowed("youtube.com").await);
        assert!(wl.is_allowed("youtube.com").await);
        assert_eq!(*repo.get_calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn fixed_endpoint_parses_domain_and_endpoint() {
        let wl = whitelist("youtube.com\nvimeo.com,https://vimeo.com/api/oembed.json");
        assert_eq!(
            wl.fixed_endpoint_for("https://youtube.com/watch?v=x").await,
            None
        );
        assert_eq!(
            wl.fixed_endpoint_for("https://vimeo.com/12345").await,
            Some("https://vimeo.com/api/oembed.json".to_string())
        );
        // ドメイン自体は後方一致でホワイトリスト判定に使える（エンドポイントの有無とは独立）。
        assert!(wl.is_allowed("player.vimeo.com").await);
    }

    #[tokio::test]
    async fn fixed_endpoint_ignores_malformed_or_missing_lines() {
        let wl = whitelist("youtube.com,\n,https://example.com\n  \nvimeo.com , https://vimeo.com/api/oembed.json ");
        assert!(wl.is_allowed("youtube.com").await);
        assert_eq!(
            wl.fixed_endpoint_for("https://vimeo.com/1").await,
            Some("https://vimeo.com/api/oembed.json".to_string())
        );
    }
}

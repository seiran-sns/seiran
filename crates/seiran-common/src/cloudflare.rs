//! Cloudflare DNS API（ハンドル検証用の `_atproto.{handle}` TXT レコード）。

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use hickory_resolver::config::{NameServerConfig, ResolverConfig, ResolverOpts};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::{RData, RecordType};
use hickory_resolver::TokioResolver;

use crate::repository::SiteSettingsRepository;

/// 権威 NS に見えてから最初のイベントを出す（PLC へ送る）までの余裕。Cloudflare の NS は anycast で、
/// ここから見えた PoP と Relay/AppView が問い合わせる PoP とで反映時刻がずれうる。
pub const TXT_PUBLISH_GRACE: Duration = Duration::from_secs(2);

const NS_CACHE_TTL: Duration = Duration::from_secs(600);
type NsCache =
    std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, Vec<String>)>>;
static NS_CACHE: std::sync::OnceLock<NsCache> = std::sync::OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub enum CloudflareError {
    #[error("HTTP エラー: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Cloudflare API エラー: {0}")]
    Api(String),
}

pub struct CloudflareClient {
    http: Arc<reqwest::Client>,
    token: String,
    zone_id: String,
}

impl CloudflareClient {
    pub fn new(http: Arc<reqwest::Client>, token: String, zone_id: String) -> Self {
        Self {
            http,
            token,
            zone_id,
        }
    }

    /// `_atproto.{handle}` TXT レコードを作成し、レコード ID を返す。
    pub async fn set_atproto_txt(
        &self,
        handle: &str,
        did: &str,
    ) -> Result<String, CloudflareError> {
        let url = format!(
            "https://api.cloudflare.com/client/v4/zones/{}/dns_records",
            self.zone_id
        );
        let resp: serde_json::Value = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(&serde_json::json!({
                "type": "TXT",
                "name": format!("_atproto.{}", handle),
                "content": format!("did={}", did),
                "ttl": 60,
            }))
            .send()
            .await?
            .json()
            .await?;

        if !resp["success"].as_bool().unwrap_or(false) {
            return Err(CloudflareError::Api(resp["errors"].to_string()));
        }

        resp["result"]["id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| CloudflareError::Api("レコード ID が取得できません".to_string()))
    }

    /// ゾーンの全権威 NS が `_atproto.{handle}` に `did={did}` を返すか。
    pub async fn is_atproto_txt_published(
        &self,
        handle: &str,
        did: &str,
    ) -> Result<bool, CloudflareError> {
        let resolvers = self.authoritative_resolvers().await?;
        txt_published_on_all(&resolvers, handle, did).await
    }

    /// ゾーンの権威 NS ごとの、キャッシュを持たないリゾルバー。1つのリゾルバーに複数の
    /// NS を渡すと応答の速い1台にしか聞かないため、NS ごとに分ける。IPv6 を持たない
    /// コンテナ環境で到達できず待ちが必ずタイムアウトしないよう、IPv4 だけを使う。
    async fn authoritative_resolvers(&self) -> Result<Vec<TokioResolver>, CloudflareError> {
        let mut resolvers = Vec::new();
        for ns in self.zone_name_servers().await? {
            let addrs = match tokio::net::lookup_host((ns.as_str(), 53)).await {
                Ok(addrs) => addrs,
                Err(e) => {
                    tracing::warn!("[cloudflare] 権威 NS の名前解決に失敗: {}: {}", ns, e);
                    continue;
                }
            };
            let Some(ip) = addrs.map(|a| a.ip()).find(IpAddr::is_ipv4) else {
                continue;
            };
            let mut opts = ResolverOpts::default();
            opts.cache_size = 0;
            opts.attempts = 1;
            opts.timeout = Duration::from_secs(2);
            opts.recursion_desired = false;
            let config = ResolverConfig::from_name_servers(vec![NameServerConfig::udp(ip)]);
            match TokioResolver::builder_with_config(config, TokioRuntimeProvider::default())
                .with_options(opts)
                .build()
            {
                Ok(r) => resolvers.push(r),
                Err(e) => tracing::warn!("[cloudflare] リゾルバー作成に失敗: {}: {}", ns, e),
            }
        }
        Ok(resolvers)
    }

    /// Cloudflare がこのゾーンに割り当てた権威 NS のホスト名。公開確認は登録ごとに秒単位で
    /// 繰り返すので、API のレート制限に掛からないようゾーンごとに `NS_CACHE_TTL` だけ覚える
    /// （割り当てはゾーンを作り直さない限り変わらない）。
    async fn zone_name_servers(&self) -> Result<Vec<String>, CloudflareError> {
        let cache = NS_CACHE.get_or_init(Default::default);
        if let Some((fetched, ns)) = cache
            .lock()
            .ok()
            .and_then(|c| c.get(&self.zone_id).cloned())
        {
            if fetched.elapsed() < NS_CACHE_TTL {
                return Ok(ns);
            }
        }
        let ns = self.fetch_zone_name_servers().await?;
        if let Ok(mut c) = cache.lock() {
            c.insert(
                self.zone_id.clone(),
                (std::time::Instant::now(), ns.clone()),
            );
        }
        Ok(ns)
    }

    async fn fetch_zone_name_servers(&self) -> Result<Vec<String>, CloudflareError> {
        let url = format!(
            "https://api.cloudflare.com/client/v4/zones/{}",
            self.zone_id
        );
        let resp: serde_json::Value = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await?
            .json()
            .await?;
        if !resp["success"].as_bool().unwrap_or(false) {
            return Err(CloudflareError::Api(resp["errors"].to_string()));
        }
        Ok(resp["result"]["name_servers"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// `_atproto.{handle}` TXT レコードが存在しない場合のみ作成する。
    /// 既存レコードがあれば何もしない。
    pub async fn ensure_atproto_txt(&self, handle: &str, did: &str) -> Result<(), CloudflareError> {
        let name = format!("_atproto.{}", handle);
        let list_url = format!(
            "https://api.cloudflare.com/client/v4/zones/{}/dns_records?type=TXT&name={}",
            self.zone_id, name
        );
        let list_resp: serde_json::Value = self
            .http
            .get(&list_url)
            .bearer_auth(&self.token)
            .send()
            .await?
            .json()
            .await?;

        let existing = list_resp["result"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        if existing {
            return Ok(());
        }

        self.set_atproto_txt(handle, did).await.map(|_| ())
    }

    /// TXT レコードを削除する。
    pub async fn delete_txt_record(&self, record_id: &str) -> Result<(), CloudflareError> {
        let url = format!(
            "https://api.cloudflare.com/client/v4/zones/{}/dns_records/{}",
            self.zone_id, record_id
        );
        let resp: serde_json::Value = self
            .http
            .delete(&url)
            .bearer_auth(&self.token)
            .send()
            .await?
            .json()
            .await?;

        if !resp["success"].as_bool().unwrap_or(false) {
            return Err(CloudflareError::Api(resp["errors"].to_string()));
        }
        Ok(())
    }
}

/// 公開リゾルバーに聞くと、公開前の問い合わせで NXDOMAIN をキャッシュさせてしまうため、
/// 権威 NS へキャッシュ無しで直接問い合わせる。
async fn txt_published_on_all(
    resolvers: &[TokioResolver],
    handle: &str,
    did: &str,
) -> Result<bool, CloudflareError> {
    if resolvers.is_empty() {
        return Err(CloudflareError::Api(
            "権威 NS のアドレスが得られません".to_string(),
        ));
    }
    let name = format!("_atproto.{}.", handle);
    let expected = format!("did={}", did);
    for resolver in resolvers {
        if !txt_matches(resolver, &name, &expected).await {
            return Ok(false);
        }
    }
    Ok(true)
}

/// 管理画面設定（`cloudflare_api_token` / `cloudflare_zone_id`）から Cloudflare DNS クライアントを作る。
/// 設定の変更を再起動なしで反映するため、呼び出しごとに読む。どちらかが空なら `None`
/// （ハンドル検証は HTTP `/.well-known/atproto-did` のみになる）。
pub async fn client_from_settings(
    settings: &dyn SiteSettingsRepository,
    http: &Arc<reqwest::Client>,
) -> Option<Arc<CloudflareClient>> {
    let settings = match settings.get_all().await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[cloudflare] 設定の取得に失敗: {}", e);
            return None;
        }
    };
    let value = |key: &str| {
        settings
            .get(key)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
    };
    let (token, zone_id) = (value("cloudflare_api_token")?, value("cloudflare_zone_id")?);
    Some(Arc::new(CloudflareClient::new(
        Arc::clone(http),
        token.to_string(),
        zone_id.to_string(),
    )))
}

/// `name` の TXT に `expected` と一致する値があるか。NXDOMAIN・タイムアウトは「まだ」とみなす。
async fn txt_matches(resolver: &TokioResolver, name: &str, expected: &str) -> bool {
    let Ok(lookup) = resolver.lookup(name, RecordType::TXT).await else {
        return false;
    };
    lookup.answers().iter().any(|record| match &record.data {
        RData::TXT(txt) => {
            let value: Vec<u8> = txt
                .txt_data
                .iter()
                .flat_map(|c| c.iter().copied())
                .collect();
            value == expected.as_bytes()
        }
        _ => false,
    })
}

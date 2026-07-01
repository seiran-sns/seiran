//! Webfinger 解決モジュール (Fediverse アクター特定用)
//!
//! ユーザー識別子 `acct:username@domain` を入力として、
//! 対応するリモートサーバーの Webfinger エンドポイントを叩き、
//! ActivityPub の Actor URI (href) を取得する。

use serde::{Deserialize, Serialize};

use super::client::ApError;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WebFingerLink {
    pub rel: String,
    #[serde(rename = "type")]
    pub mime_type: Option<String>,
    pub href: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WebFingerResponse {
    pub subject: String,
    pub aliases: Option<Vec<String>>,
    pub links: Vec<WebFingerLink>,
}

impl WebFingerResponse {
    /// ActivityPub の Actor URI (`rel="self"`, `type="application/activity+json"` もしくは `"application/ld+json; profile=\"https://www.w3.org/ns/activitystreams\""`) を取り出す
    pub fn actor_uri(&self) -> Option<String> {
        self.links.iter().find(|link| {
            link.rel == "self" && link.mime_type.as_ref().map(|t| {
                t.contains("application/activity+json") || t.contains("application/ld+json")
            }).unwrap_or(false)
        }).and_then(|link| link.href.clone())
    }
}

/// Webfinger 解決の内部実装（ApClient::resolve_webfinger から呼ばれる）
pub(super) async fn resolve_webfinger_impl(client: &reqwest::Client, username: &str, domain: &str) -> Result<String, ApError> {
    let resource = format!("acct:{}@{}", username, domain);
    let url = format!(
        "https://{}/.well-known/webfinger?resource={}",
        domain,
        urlencoding::encode(&resource)
    );

    println!("[Webfinger] 解決を試行中: {}", url);

    let res = client
        .get(&url)
        .header("Accept", "application/jrd+json, application/json")
        .send()
        .await?;

    if !res.status().is_success() {
        return Err(ApError::Other(format!("Webfinger応答エラー: ステータス {}", res.status())));
    }

    let parsed = res.json::<WebFingerResponse>().await?;

    parsed
        .actor_uri()
        .ok_or_else(|| ApError::Other("Webfinger リンクに ActivityPub 互換アクターURIが見つかりません".to_string()))
}

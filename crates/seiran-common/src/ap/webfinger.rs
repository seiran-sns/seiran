//! Webfinger 解決モジュール (Fediverse アクター特定用)
//!
//! ユーザー識別子 `acct:username@domain` を入力として、
//! 対応するリモートサーバーの Webfinger エンドポイントを叩き、
//! ActivityPub の Actor URI (href) を取得する。

use serde::{Deserialize, Serialize};

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

/// 指定したアクター名とドメインから Webfinger 解決を実行する
pub async fn resolve_webfinger(username: &str, domain: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent("seiran-federation/0.1.0")
        .build()
        .map_err(|e| format!("HTTPクライアント初期化失敗: {}", e))?;

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
        .await
        .map_err(|e| format!("Webfingerリクエスト失敗: {}", e))?;

    if !res.status().is_success() {
        return Err(format!("Webfinger応答エラー: ステータス {}", res.status()));
    }

    let parsed = res
        .json::<WebFingerResponse>()
        .await
        .map_err(|e| format!("Webfinger JSONパース失敗: {}", e))?;

    parsed
        .actor_uri()
        .ok_ok_or_else(|| "Webfinger リンクに ActivityPub 互換アクターURIが見つかりません".to_string())
}

trait OkOrExt<T> {
    fn ok_ok_or_else<F: FnOnce() -> String>(self, err_fn: F) -> Result<T, String>;
}

impl<T> OkOrExt<T> for Option<T> {
    fn ok_ok_or_else<F: FnOnce() -> String>(self, err_fn: F) -> Result<T, String> {
        self.ok_or_else(err_fn)
    }
}

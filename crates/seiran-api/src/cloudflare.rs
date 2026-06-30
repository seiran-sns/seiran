use std::sync::Arc;

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
        Self { http, token, zone_id }
    }

    /// `_atproto.{handle}` TXT レコードを作成し、レコード ID を返す。
    pub async fn set_atproto_txt(&self, handle: &str, did: &str) -> Result<String, CloudflareError> {
        let url = format!(
            "https://api.cloudflare.com/client/v4/zones/{}/dns_records",
            self.zone_id
        );
        let resp: serde_json::Value = self.http
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

    /// `_atproto.{handle}` TXT レコードが存在しない場合のみ作成する。
    /// 既存レコードがあれば何もしない。
    pub async fn ensure_atproto_txt(&self, handle: &str, did: &str) -> Result<(), CloudflareError> {
        let name = format!("_atproto.{}", handle);
        let list_url = format!(
            "https://api.cloudflare.com/client/v4/zones/{}/dns_records?type=TXT&name={}",
            self.zone_id, name
        );
        let list_resp: serde_json::Value = self.http
            .get(&list_url)
            .bearer_auth(&self.token)
            .send()
            .await?
            .json()
            .await?;

        let existing = list_resp["result"].as_array().map(|a| !a.is_empty()).unwrap_or(false);
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
        let resp: serde_json::Value = self.http
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

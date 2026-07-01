use aws_sdk_s3::{
    config::{BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client, Config,
};

use crate::repository::StorageProvider;

#[derive(Debug, thiserror::Error)]
pub enum S3Error {
    #[error("アップロード失敗: {0}")]
    Put(String),
    #[error("削除失敗: {0}")]
    Delete(String),
}

pub struct S3StorageClient {
    client: Client,
    bucket: String,
    public_url: String,
}

impl S3StorageClient {
    pub fn new(provider: &StorageProvider) -> Self {
        let credentials = Credentials::new(
            &provider.access_key,
            &provider.secret_key,
            None,
            None,
            "seiran",
        );
        let config = Config::builder()
            .endpoint_url(&provider.endpoint)
            .region(Region::new(provider.region.clone()))
            .credentials_provider(credentials)
            .force_path_style(true)
            .behavior_version(BehaviorVersion::latest())
            .build();
        Self {
            client: Client::from_conf(config),
            bucket: provider.bucket.clone(),
            public_url: provider.public_url.trim_end_matches('/').to_owned(),
        }
    }

    /// オブジェクトをアップロードし、公開 URL を返す。
    pub async fn put(&self, key: &str, data: Vec<u8>, content_type: &str) -> Result<String, S3Error> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(data))
            .content_type(content_type)
            .send()
            .await
            .map_err(|e| S3Error::Put(e.to_string()))?;

        Ok(format!("{}/{}", self.public_url, key))
    }

    /// オブジェクトを削除する。オブジェクトが存在しない場合もエラーにしない。
    pub async fn delete(&self, key: &str) -> Result<(), S3Error> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| S3Error::Delete(e.to_string()))?;
        Ok(())
    }
}

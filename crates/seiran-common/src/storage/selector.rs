use crate::repository::{StorageProvider, StorageProviderError, StorageProviderRepository};

#[derive(Debug, thiserror::Error)]
pub enum SelectorError {
    #[error("利用可能なストレージプロバイダーがありません（全プロバイダーが容量超過または無効）")]
    NoAvailableProvider,
    #[error("ストレージ選択 DB エラー: {0}")]
    Db(#[from] StorageProviderError),
}

/// アクティブなプロバイダーを id 昇順でスキャンし、
/// `capacity_mb` の残容量に `file_size` が収まる最初のものを返す。
/// `capacity_mb` が NULL のプロバイダーは無制限として即採用する。
pub async fn select_provider(
    repo: &dyn StorageProviderRepository,
    file_size: i64,
) -> Result<StorageProvider, SelectorError> {
    let providers = repo.list_active().await?;

    for provider in providers {
        match provider.capacity_mb {
            None => return Ok(provider),
            Some(cap_mb) => {
                let used = repo.get_used_bytes(provider.id).await?;
                if used + file_size <= cap_mb * 1024 * 1024 {
                    return Ok(provider);
                }
            }
        }
    }

    Err(SelectorError::NoAvailableProvider)
}

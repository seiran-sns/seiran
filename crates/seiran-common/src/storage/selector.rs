use crate::repository::{StorageProvider, StorageProviderError, StorageProviderRepository};

#[derive(Debug, thiserror::Error)]
pub enum SelectorError {
    /// アクティブなストレージプロバイダーが1件も設定されていない
    #[error("利用可能なストレージプロバイダーがありません（未設定または全て無効）")]
    NoAvailableProvider,
    /// プロバイダーは存在するが、全て capacity_mb の上限に達している
    #[error("全てのストレージプロバイダーが容量上限に達しています")]
    QuotaExceeded,
    #[error("ストレージ選択 DB エラー: {0}")]
    Db(#[from] StorageProviderError),
}

/// アクティブなプロバイダーを id 昇順でスキャンし、
/// `capacity_mb` の残容量に `file_size` が収まる最初のものを返す。
/// `capacity_mb` が NULL のプロバイダーは無制限として即採用する。
///
/// プロバイダーが1件も設定されていない場合は `NoAvailableProvider`、
/// 全プロバイダーが容量超過の場合は `QuotaExceeded` を返す。
pub async fn select_provider(
    repo: &dyn StorageProviderRepository,
    file_size: i64,
) -> Result<StorageProvider, SelectorError> {
    let providers = repo.list_active().await?;

    if providers.is_empty() {
        return Err(SelectorError::NoAvailableProvider);
    }

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

    // プロバイダーは存在したが全て容量超過
    Err(SelectorError::QuotaExceeded)
}

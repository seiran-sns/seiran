#[derive(Debug, thiserror::Error)]
pub enum FederationError {
    #[error("DB エラー: {0}")]
    Db(#[from] sqlx::Error),
    #[error("AP エラー: {0}")]
    Ap(String),
    #[error("フィールド不足: {0}")]
    MissingField(&'static str),
    #[error("署名検証失敗")]
    SignatureInvalid,
}

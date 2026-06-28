pub mod auth0;
pub mod local;

pub use auth0::Auth0Provider;
pub use local::LocalAuthProvider;

use std::sync::Arc;
use crate::traits::AuthProvider;
use crate::secrets::Secrets;

/// 環境変数と Secrets に基づいて適切な AuthProvider 実装を生成します。
///
/// JWT_SECRET は Secrets から自動的に取得されるため、
/// ユーザーが環境変数で設定する必要はありません。
pub fn create_auth_provider(secrets: &Secrets) -> Arc<dyn AuthProvider> {
    let provider_type = std::env::var("AUTH_PROVIDER").unwrap_or_else(|_| "local".to_string());

    match provider_type.as_str() {
        "auth0" => {
            let audience = std::env::var("AUTH0_AUDIENCE")
                .unwrap_or_else(|_| "https://seiran.org/api".to_string());
            let issuer = std::env::var("AUTH0_ISSUER")
                .unwrap_or_else(|_| "https://seiran.us.auth0.com/".to_string());
            Arc::new(Auth0Provider::new(audience, issuer))
        }
        _ => {
            // JWT_SECRET は Secrets から取得（環境変数不要）
            let secret_bytes = secrets.jwt_secret_bytes();
            Arc::new(LocalAuthProvider::new(secret_bytes))
        }
    }
}

//! Mastodon 互換 API の OAuth 2.0（アプリ登録・認可コード・発行トークンの記録）。
//!
//! client_secret と認可コードは呼び出し側で SHA-256（hex）にしてから渡す（平文は保存しない、
//! `oauth_apps` マイグレーション参照）。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// 登録済み OAuth アプリ。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OAuthAppRow {
    pub id: i64,
    pub client_id: String,
    pub client_secret_hash: String,
    pub name: String,
    pub redirect_uris: Vec<String>,
    pub scopes: String,
    pub website: Option<String>,
}

/// `insert_app` の入力。
pub struct NewOAuthApp<'a> {
    pub id: i64,
    pub client_id: &'a str,
    pub client_secret_hash: &'a str,
    pub name: &'a str,
    pub redirect_uris: &'a [String],
    pub scopes: &'a str,
    pub website: Option<&'a str>,
}

pub async fn insert_app(pool: &PgPool, app: &NewOAuthApp<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO oauth_apps (id, client_id, client_secret_hash, name, redirect_uris, scopes, website)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(app.id)
    .bind(app.client_id)
    .bind(app.client_secret_hash)
    .bind(app.name)
    .bind(app.redirect_uris)
    .bind(app.scopes)
    .bind(app.website)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn find_app_by_client_id(
    pool: &PgPool,
    client_id: &str,
) -> Result<Option<OAuthAppRow>, sqlx::Error> {
    sqlx::query_as::<_, OAuthAppRow>(
        "SELECT id, client_id, client_secret_hash, name, redirect_uris, scopes, website
         FROM oauth_apps WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_optional(pool)
    .await
}

/// 発行済みトークン（`app_tokens.id` = JWT の jti）がどのアプリのものか。OAuth 以外
/// （MiAuth・自社ログイン）で発行されたトークンなら `None`。
pub async fn find_app_by_token_id(
    pool: &PgPool,
    token_id: Uuid,
) -> Result<Option<OAuthAppRow>, sqlx::Error> {
    sqlx::query_as::<_, OAuthAppRow>(
        "SELECT o.id, o.client_id, o.client_secret_hash, o.name, o.redirect_uris, o.scopes, o.website
         FROM app_tokens t JOIN oauth_apps o ON o.id = t.oauth_app_id
         WHERE t.id = $1",
    )
    .bind(token_id)
    .fetch_optional(pool)
    .await
}

/// `insert_authorization_code` の入力。
pub struct NewAuthorizationCode<'a> {
    pub code_hash: &'a str,
    pub app_id: i64,
    pub user_id: i64,
    pub redirect_uri: &'a str,
    pub scopes: &'a str,
    pub code_challenge: Option<&'a str>,
    pub code_challenge_method: Option<&'a str>,
    pub expires_at: DateTime<Utc>,
}

/// 認可コードを保存する。期限切れのまま交換されなかったコードもここで掃除する
/// （交換されないコードは他に消す契機が無いため）。
pub async fn insert_authorization_code(
    pool: &PgPool,
    code: &NewAuthorizationCode<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM oauth_authorization_codes WHERE expires_at < now()")
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO oauth_authorization_codes
             (code_hash, app_id, user_id, redirect_uri, scopes, code_challenge, code_challenge_method, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(code.code_hash)
    .bind(code.app_id)
    .bind(code.user_id)
    .bind(code.redirect_uri)
    .bind(code.scopes)
    .bind(code.code_challenge)
    .bind(code.code_challenge_method)
    .bind(code.expires_at)
    .execute(pool)
    .await
    .map(|_| ())
}

/// 消費した認可コードの中身。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConsumedAuthorizationCode {
    pub app_id: i64,
    pub user_id: i64,
    /// トークン（JWT）の `email` クレーム用。
    pub email: String,
    pub redirect_uri: String,
    pub scopes: String,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
}

/// 期限内の認可コードを1文で取り出して消す。同じコードを並行して2回交換しようとしても
/// 片方しか行を得られない。期限切れ・存在しないコードは `None`。
pub async fn consume_authorization_code(
    pool: &PgPool,
    code_hash: &str,
    app_id: i64,
) -> Result<Option<ConsumedAuthorizationCode>, sqlx::Error> {
    sqlx::query_as::<_, ConsumedAuthorizationCode>(
        "DELETE FROM oauth_authorization_codes c USING users u
         WHERE c.code_hash = $1 AND c.app_id = $2 AND c.expires_at >= now() AND u.id = c.user_id
         RETURNING c.app_id, c.user_id, u.email, c.redirect_uri, c.scopes,
                   c.code_challenge, c.code_challenge_method",
    )
    .bind(code_hash)
    .bind(app_id)
    .fetch_optional(pool)
    .await
}

/// OAuth で発行したトークンを `app_tokens` に記録する（設定画面の「連携アプリ」一覧・
/// 無効化の対象になる。MiAuth 発行分と同じテーブル）。
pub async fn insert_app_token(
    pool: &PgPool,
    token_id: Uuid,
    user_id: i64,
    client_name: &str,
    app_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO app_tokens (id, user_id, client_name, oauth_app_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(token_id)
    .bind(user_id)
    .bind(client_name)
    .bind(app_id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// `POST /oauth/revoke`。トークンを発行したアプリ自身からの取り消しに限る（他アプリの
/// client_id では取り消せない）。取り消せたら true。
pub async fn revoke_app_token(
    pool: &PgPool,
    token_id: Uuid,
    app_id: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE app_tokens SET revoked_at = now()
         WHERE id = $1 AND oauth_app_id = $2 AND revoked_at IS NULL",
    )
    .bind(token_id)
    .bind(app_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

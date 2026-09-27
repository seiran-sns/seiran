//! OAuth 2.0（Mastodon 互換）と Misskey 旧来 app 認証フローの共有アプリ登録
//! （アプリ登録・認可コード or 認可セッション・発行トークンの記録）。
//!
//! client_secret と認可コード/セッションは呼び出し側で SHA-256（hex）にしてから渡す
//! （平文は保存しない、`oauth_apps` マイグレーション参照）。

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// 推測不能なランダム文字列（UUIDv4 2つ分、244ビット）。
pub fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// 登録済み OAuth アプリ。Misskey の `app/create` はこれを `description` 付きで
/// 登録し、`client_id` は内部識別用のみ（クライアントには返さない）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OAuthAppRow {
    pub id: i64,
    pub client_id: String,
    pub client_secret_hash: String,
    pub name: String,
    pub redirect_uris: Vec<String>,
    pub scopes: String,
    pub website: Option<String>,
    pub description: Option<String>,
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
    pub description: Option<&'a str>,
}

pub async fn insert_app(pool: &PgPool, app: &NewOAuthApp<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO oauth_apps (id, client_id, client_secret_hash, name, redirect_uris, scopes, website, description)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(app.id)
    .bind(app.client_id)
    .bind(app.client_secret_hash)
    .bind(app.name)
    .bind(app.redirect_uris)
    .bind(app.scopes)
    .bind(app.website)
    .bind(app.description)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn find_app_by_client_id(
    pool: &PgPool,
    client_id: &str,
) -> Result<Option<OAuthAppRow>, sqlx::Error> {
    sqlx::query_as::<_, OAuthAppRow>(
        "SELECT id, client_id, client_secret_hash, name, redirect_uris, scopes, website, description
         FROM oauth_apps WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_optional(pool)
    .await
}

/// Misskey の `auth/session/generate`・`auth/session/userkey` は client_id を送らず
/// appSecret 単体でアプリを特定する。
pub async fn find_app_by_client_secret_hash(
    pool: &PgPool,
    client_secret_hash: &str,
) -> Result<Option<OAuthAppRow>, sqlx::Error> {
    sqlx::query_as::<_, OAuthAppRow>(
        "SELECT id, client_id, client_secret_hash, name, redirect_uris, scopes, website, description
         FROM oauth_apps WHERE client_secret_hash = $1",
    )
    .bind(client_secret_hash)
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
        "SELECT o.id, o.client_id, o.client_secret_hash, o.name, o.redirect_uris, o.scopes, o.website, o.description
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

/// OAuth・Misskey 旧来 app 認証フローで発行したトークンを `app_tokens` に記録する
/// （設定画面の「連携アプリ」一覧・無効化の対象になる。MiAuth 発行分と同じテーブル）。
/// `misskey_hash` は Misskey クライアント（misskey4j 等）が `i` として送ってくる
/// `sha256(accessToken + appSecret)` の値（`extract_auth` のフォールバック照合用）。
/// Mastodon 互換 OAuth 発行分は `None`。
pub async fn insert_app_token(
    pool: &PgPool,
    token_id: Uuid,
    user_id: i64,
    client_name: &str,
    app_id: i64,
    misskey_hash: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO app_tokens (id, user_id, client_name, oauth_app_id, misskey_hash)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(token_id)
    .bind(user_id)
    .bind(client_name)
    .bind(app_id)
    .bind(misskey_hash)
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

/// `insert_misskey_auth_session` の入力。
pub struct NewMisskeyAuthSession<'a> {
    pub token_hash: &'a str,
    pub app_id: i64,
    pub expires_at: DateTime<Utc>,
}

/// Misskey `auth/session/generate` が発行する認可セッションを保存する。期限切れのまま
/// 承認されなかったセッションもここで掃除する（承認されないセッションは他に消す契機が無い
/// ため）。
pub async fn insert_misskey_auth_session(
    pool: &PgPool,
    session: &NewMisskeyAuthSession<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM misskey_auth_sessions WHERE expires_at < now()")
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO misskey_auth_sessions (token_hash, app_id, expires_at) VALUES ($1, $2, $3)",
    )
    .bind(session.token_hash)
    .bind(session.app_id)
    .bind(session.expires_at)
    .execute(pool)
    .await
    .map(|_| ())
}

/// 承認確認画面（アプリ名表示用）が使う、非消費の参照。期限切れ・不明なら `None`。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MisskeyAuthSessionInfo {
    pub app_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub scopes: String,
    pub redirect_uris: Vec<String>,
}

pub async fn find_misskey_auth_session(
    pool: &PgPool,
    token_hash: &str,
) -> Result<Option<MisskeyAuthSessionInfo>, sqlx::Error> {
    sqlx::query_as::<_, MisskeyAuthSessionInfo>(
        "SELECT o.id AS app_id, o.name, o.description, o.scopes, o.redirect_uris
         FROM misskey_auth_sessions s JOIN oauth_apps o ON o.id = s.app_id
         WHERE s.token_hash = $1 AND s.expires_at >= now()",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await
}

/// ユーザーが承認確認画面で「承認する」を押した時に呼ぶ。再承認（未消費の間の複数回呼び出し）
/// は同じユーザーであれば冪等に許容する。最終的な安全性は `consume_misskey_auth_session` の
/// 一度きり消費が担保する。
pub async fn approve_misskey_auth_session(
    pool: &PgPool,
    token_hash: &str,
    user_id: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE misskey_auth_sessions SET user_id = $2
         WHERE token_hash = $1 AND expires_at >= now() AND (user_id IS NULL OR user_id = $2)",
    )
    .bind(token_hash)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 消費した Misskey 認可セッションの中身。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConsumedMisskeyAuthSession {
    pub user_id: i64,
    /// トークン（JWT）の `email` クレーム用。
    pub email: String,
}

/// `auth/session/userkey`。承認済み（user_id が確定した）セッションを1文で取り出して消す。
/// 未承認・期限切れ・存在しないセッションは `None`。
pub async fn consume_misskey_auth_session(
    pool: &PgPool,
    token_hash: &str,
    app_id: i64,
) -> Result<Option<ConsumedMisskeyAuthSession>, sqlx::Error> {
    sqlx::query_as::<_, ConsumedMisskeyAuthSession>(
        "DELETE FROM misskey_auth_sessions s USING users u
         WHERE s.token_hash = $1 AND s.app_id = $2 AND s.user_id IS NOT NULL
           AND s.expires_at >= now() AND u.id = s.user_id
         RETURNING s.user_id, u.email",
    )
    .bind(token_hash)
    .bind(app_id)
    .fetch_optional(pool)
    .await
}

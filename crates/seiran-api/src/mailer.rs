//! SMTP メール送信ユーティリティ
//!
//! SMTP 設定は DB の `site_settings` テーブルから取得する。
//! キー: `smtp_host`, `smtp_port`, `smtp_username`, `smtp_password`, `smtp_from`, `smtp_tls`
//!
//! 件名・本文は `locales/<lang>/mail.json` から引く（`crate::i18n`）。どの関数も送る言語を
//! 引数に取り、言語の決め方は呼び出し側が `i18n::request_language` /
//! `i18n::stored_language` で決める。

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::collections::HashMap;
use std::time::Duration;

use crate::i18n::Namespace;

/// メールの文言（`locales/<lang>/mail.json`）。
fn text(language: &str, key: &str, vars: &[(&str, &str)]) -> String {
    crate::i18n::text(Namespace::Mail, language, key, vars)
}

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("SMTP が未設定です: {0}")]
    Config(&'static str),
    #[error("メッセージ構築失敗: {0}")]
    Build(#[from] lettre::error::Error),
    #[error("SMTP 送信失敗: {0}")]
    Send(#[from] lettre::transport::smtp::Error),
    #[error("アドレス解析失敗: {0}")]
    Address(#[from] lettre::address::AddressError),
    #[error("SMTP サーバーが NOOP に応答しません")]
    NotConnected,
}

/// 管理画面の接続確認・テスト送信の待ち時間。lettre の既定（60秒）のままだと、
/// ホスト名やポートを間違えたときに結果が出るまで管理画面が1分待たされる。
const TEST_TIMEOUT: Duration = Duration::from_secs(15);

/// SMTPが設定済みかどうかを判定する（`build_transport`と同じ判定基準）。
/// ATPセッション2FA（`com.atproto.server.createSession`のauthFactorToken）は、
/// SMTP未設定のインスタンスではコード送信自体が不可能なため、この判定で2FAを
/// 常にスキップする。
pub fn is_smtp_configured(settings: &HashMap<String, String>) -> bool {
    settings.get("smtp_host").is_some_and(|v| !v.is_empty())
}

/// DB から取得した site_settings のマップから SMTP トランスポートを構築する。
/// `smtp_host` が存在しない場合は `MailError::Config` を返す。
fn build_transport(
    settings: &HashMap<String, String>,
) -> Result<(AsyncSmtpTransport<Tokio1Executor>, String), MailError> {
    build_transport_with_timeout(settings, None)
}

/// `timeout` が `None` なら lettre の既定の待ち時間を使う。
fn build_transport_with_timeout(
    settings: &HashMap<String, String>,
    timeout: Option<Duration>,
) -> Result<(AsyncSmtpTransport<Tokio1Executor>, String), MailError> {
    let host = settings
        .get("smtp_host")
        .filter(|v| !v.is_empty())
        .ok_or(MailError::Config("smtp_host"))?
        .clone();
    let port: u16 = settings
        .get("smtp_port")
        .and_then(|v| v.parse().ok())
        .unwrap_or(587);
    let username = settings
        .get("smtp_username")
        .filter(|v| !v.is_empty())
        .ok_or(MailError::Config("smtp_username"))?
        .clone();
    let password = settings
        .get("smtp_password")
        .filter(|v| !v.is_empty())
        .ok_or(MailError::Config("smtp_password"))?
        .clone();
    let from = settings
        .get("smtp_from")
        .filter(|v| !v.is_empty())
        .ok_or(MailError::Config("smtp_from"))?
        .clone();
    let tls_mode = settings
        .get("smtp_tls")
        .cloned()
        .unwrap_or_else(|| "starttls".to_string());

    let creds = Credentials::new(username, password);

    let builder = match tls_mode.as_str() {
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&host)?,
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)?,
    }
    .port(port)
    .credentials(creds);
    let builder = match timeout {
        Some(t) => builder.timeout(Some(t)),
        None => builder,
    };
    let transport = builder.build();

    Ok((transport, from))
}

/// 管理画面の接続確認。接続・STARTTLS（または TLS）・認証まで行い、NOOP の応答を見る。
pub async fn test_smtp_connection(settings: &HashMap<String, String>) -> Result<(), MailError> {
    let (transport, _) = build_transport_with_timeout(settings, Some(TEST_TIMEOUT))?;
    if transport.test_connection().await? {
        Ok(())
    } else {
        Err(MailError::NotConnected)
    }
}

/// `to` へ件名・本文だけのプレーンテキストのメールを送る。
async fn send_plain(
    transport: &AsyncSmtpTransport<Tokio1Executor>,
    from: &str,
    to: &str,
    subject: String,
    body: String,
) -> Result<(), MailError> {
    let email = Message::builder()
        .from(from.parse()?)
        .to(to.trim().parse()?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body)?;
    transport.send(email).await?;
    Ok(())
}

/// 言語ファイルの `{key}.subject` / `{key}.body` で組み立てたメールを送る。
async fn send_message(
    settings: &HashMap<String, String>,
    language: &str,
    to: &str,
    key: &str,
    vars: &[(&str, &str)],
) -> Result<(), MailError> {
    let (transport, from) = build_transport(settings)?;
    send_plain(
        &transport,
        &from,
        to,
        text(language, &format!("{key}.subject"), vars),
        text(language, &format!("{key}.body"), vars),
    )
    .await
}

/// 管理画面のテスト送信。設定どおりの差出人から `to` へ短い本文を送る。
pub async fn send_test_email(
    settings: &HashMap<String, String>,
    language: &str,
    to: &str,
) -> Result<(), MailError> {
    let (transport, from) = build_transport_with_timeout(settings, Some(TEST_TIMEOUT))?;
    send_plain(
        &transport,
        &from,
        to,
        text(language, "test.subject", &[]),
        text(language, "test.body", &[]),
    )
    .await
}

pub async fn send_verification_email(
    settings: &HashMap<String, String>,
    language: &str,
    to: &str,
    verify_url: &str,
) -> Result<(), MailError> {
    send_message(
        settings,
        language,
        to,
        "verifyEmail",
        &[("url", verify_url)],
    )
    .await
}

pub async fn send_email_change_confirmation(
    settings: &HashMap<String, String>,
    language: &str,
    to: &str,
    confirm_url: &str,
) -> Result<(), MailError> {
    send_message(
        settings,
        language,
        to,
        "emailChange",
        &[("url", confirm_url)],
    )
    .await
}

/// TOTP（#65）: 認証アプリ・リカバリーコードを両方失ったユーザー向けの強制解除メール。
pub async fn send_totp_disable_email(
    settings: &HashMap<String, String>,
    language: &str,
    to: &str,
    disable_url: &str,
) -> Result<(), MailError> {
    send_message(
        settings,
        language,
        to,
        "totpDisable",
        &[("url", disable_url)],
    )
    .await
}

pub async fn send_password_reset_email(
    settings: &HashMap<String, String>,
    language: &str,
    to: &str,
    reset_url: &str,
) -> Result<(), MailError> {
    send_message(
        settings,
        language,
        to,
        "passwordReset",
        &[("url", reset_url)],
    )
    .await
}

/// `com.atproto.server.createSession`（ATPセッション作成）の`authFactorToken`確認コード。
pub async fn send_atp_session_2fa_code(
    settings: &HashMap<String, String>,
    language: &str,
    to: &str,
    code: &str,
) -> Result<(), MailError> {
    send_message(settings, language, to, "atpLoginCode", &[("code", code)]).await
}

/// `com.atproto.identity.requestPlcOperationSignature`のPLCオペレーション署名確認コード。
pub async fn send_plc_operation_signature_code(
    settings: &HashMap<String, String>,
    language: &str,
    to: &str,
    code: &str,
) -> Result<(), MailError> {
    send_message(
        settings,
        language,
        to,
        "plcOperationCode",
        &[("code", code)],
    )
    .await
}

/// DID 転入の入力待ちの種類ごとの、案内メールで伝える「次にやること」の文言キー。
fn migration_input_wait_action_key(status: &str) -> &'static str {
    match status {
        "awaiting_user_confirmation" => "migration.action.awaitingUserConfirmation",
        "awaiting_plc_token" => "migration.action.awaitingPlcToken",
        "awaiting_source_password" => "migration.action.awaitingSourcePassword",
        _ => "migration.action.error",
    }
}

fn format_deadline(deadline: chrono::DateTime<chrono::Utc>) -> String {
    deadline.format("%Y-%m-%d %H:%M UTC").to_string()
}

/// DID 転入の入力待ちの案内・警告メールの中身（宛先と、本文に差し込む値）。
pub struct MigrationInputWaitMail<'a> {
    pub language: &'a str,
    pub to: &'a str,
    /// `at_migration_requests.status`（入力待ちの種類）。
    pub status: &'a str,
    pub username: &'a str,
    pub resume_url: &'a str,
    pub deadline: chrono::DateTime<chrono::Utc>,
}

/// DID 転入の入力待ちの案内（`key` が `migration.inputWait`、入力待ちに入った時点で送る）と
/// 打ち切りの警告（`migration.expiryWarning`、入力待ちのまま24時間たった時点で送る）。
async fn send_migration_input_wait_mail(
    settings: &HashMap<String, String>,
    mail: &MigrationInputWaitMail<'_>,
    key: &str,
) -> Result<(), MailError> {
    let lang = mail.language;
    let action = text(lang, migration_input_wait_action_key(mail.status), &[]);
    let resume = text(
        lang,
        "migration.resume",
        &[("url", mail.resume_url), ("username", mail.username)],
    );
    let deadline = format_deadline(mail.deadline);
    send_message(
        settings,
        lang,
        mail.to,
        key,
        &[
            ("username", mail.username),
            ("action", &action),
            ("resume", &resume),
            ("deadline", &deadline),
        ],
    )
    .await
}

/// DID 転入の入力待ちの案内（入力待ちに入った時点で送る）。
pub async fn send_migration_input_wait_notice(
    settings: &HashMap<String, String>,
    mail: &MigrationInputWaitMail<'_>,
) -> Result<(), MailError> {
    send_migration_input_wait_mail(settings, mail, "migration.inputWait").await
}

/// DID 転入の打ち切りの警告（入力待ちのまま24時間たった時点で送る）。
pub async fn send_migration_expiry_warning(
    settings: &HashMap<String, String>,
    mail: &MigrationInputWaitMail<'_>,
) -> Result<(), MailError> {
    send_migration_input_wait_mail(settings, mail, "migration.expiryWarning").await
}

/// DID 転入を期限切れで取り消した通知。
pub async fn send_migration_expired(
    settings: &HashMap<String, String>,
    language: &str,
    to: &str,
    username: &str,
) -> Result<(), MailError> {
    send_message(
        settings,
        language,
        to,
        "migration.expired",
        &[("username", username)],
    )
    .await
}

//! SMTP メール送信ユーティリティ
//!
//! SMTP 設定は DB の `site_settings` テーブルから取得する。
//! キー: `smtp_host`, `smtp_port`, `smtp_username`, `smtp_password`, `smtp_from`, `smtp_tls`

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::collections::HashMap;

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
}

/// DB から取得した site_settings のマップから SMTP トランスポートを構築する。
/// `smtp_host` が存在しない場合は `MailError::Config` を返す。
fn build_transport(
    settings: &HashMap<String, String>,
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

    let transport: AsyncSmtpTransport<Tokio1Executor> = match tls_mode.as_str() {
        "tls" => AsyncSmtpTransport::<Tokio1Executor>::relay(&host)?
            .port(port)
            .credentials(creds)
            .build(),
        _ => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)?
            .port(port)
            .credentials(creds)
            .build(),
    };

    Ok((transport, from))
}

pub async fn send_verification_email(
    settings: &HashMap<String, String>,
    to: &str,
    verify_url: &str,
) -> Result<(), MailError> {
    let (transport, from) = build_transport(settings)?;

    let body = format!(
        "seiran へようこそ。\n\n以下のリンクをクリックしてメールアドレスを確認してください:\n\n{}\n\nこのリンクは 24 時間有効です。\n\n心当たりがない場合は無視してください。",
        verify_url
    );

    let email = Message::builder()
        .from(from.parse()?)
        .to(to.parse()?)
        .subject("seiran — メールアドレスの確認")
        .header(ContentType::TEXT_PLAIN)
        .body(body)?;

    transport.send(email).await?;
    Ok(())
}

pub async fn send_email_change_confirmation(
    settings: &HashMap<String, String>,
    to: &str,
    confirm_url: &str,
) -> Result<(), MailError> {
    let (transport, from) = build_transport(settings)?;

    let body = format!(
        "seiran — メールアドレス変更のリクエストを受け付けました。\n\n以下のリンクをクリックしてこのメールアドレスへの変更を確定してください:\n\n{}\n\nこのリンクは 24 時間有効です。\n\n心当たりがない場合は無視してください。",
        confirm_url
    );

    let email = Message::builder()
        .from(from.parse()?)
        .to(to.parse()?)
        .subject("seiran — メールアドレス変更の確認")
        .header(ContentType::TEXT_PLAIN)
        .body(body)?;

    transport.send(email).await?;
    Ok(())
}

pub async fn send_password_reset_email(
    settings: &HashMap<String, String>,
    to: &str,
    reset_url: &str,
) -> Result<(), MailError> {
    let (transport, from) = build_transport(settings)?;

    let body = format!(
        "seiran — パスワードリセットのリクエストを受け付けました。\n\n以下のリンクをクリックしてパスワードを再設定してください:\n\n{}\n\nこのリンクは 1 時間有効です。\n\n心当たりがない場合は無視してください。",
        reset_url
    );

    let email = Message::builder()
        .from(from.parse()?)
        .to(to.parse()?)
        .subject("seiran — パスワードのリセット")
        .header(ContentType::TEXT_PLAIN)
        .body(body)?;

    transport.send(email).await?;
    Ok(())
}

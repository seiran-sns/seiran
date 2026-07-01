//! SMTP メール送信ユーティリティ
//!
//! 環境変数:
//! - `SMTP_HOST`     — SMTP サーバーホスト名
//! - `SMTP_PORT`     — ポート番号（デフォルト: 587）
//! - `SMTP_USERNAME` — 認証ユーザー名
//! - `SMTP_PASSWORD` — 認証パスワード
//! - `SMTP_FROM`     — 送信元アドレス（例: `info@seiran.org`）
//! - `SMTP_TLS`      — `tls`（465）/ `starttls`（587）/ `none`（デフォルト: starttls）

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("SMTP 設定が不足しています: {0}")]
    Config(&'static str),
    #[error("メッセージ構築失敗: {0}")]
    Build(#[from] lettre::error::Error),
    #[error("SMTP 送信失敗: {0}")]
    Send(#[from] lettre::transport::smtp::Error),
    #[error("アドレス解析失敗: {0}")]
    Address(#[from] lettre::address::AddressError),
}

pub async fn send_verification_email(to: &str, verify_url: &str) -> Result<(), MailError> {
    let host = std::env::var("SMTP_HOST").map_err(|_| MailError::Config("SMTP_HOST"))?;
    let port: u16 = std::env::var("SMTP_PORT")
        .unwrap_or_else(|_| "587".to_string())
        .parse()
        .unwrap_or(587);
    let username = std::env::var("SMTP_USERNAME").map_err(|_| MailError::Config("SMTP_USERNAME"))?;
    let password = std::env::var("SMTP_PASSWORD").map_err(|_| MailError::Config("SMTP_PASSWORD"))?;
    let from = std::env::var("SMTP_FROM").map_err(|_| MailError::Config("SMTP_FROM"))?;
    let tls_mode = std::env::var("SMTP_TLS").unwrap_or_else(|_| "starttls".to_string());

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

    transport.send(email).await?;
    Ok(())
}

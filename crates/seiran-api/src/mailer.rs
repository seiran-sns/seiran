//! SMTP メール送信ユーティリティ
//!
//! SMTP 設定は DB の `site_settings` テーブルから取得する。
//! キー: `smtp_host`, `smtp_port`, `smtp_username`, `smtp_password`, `smtp_from`, `smtp_tls`

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::collections::HashMap;
use std::time::Duration;

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

/// 管理画面のテスト送信。設定どおりの差出人から `to` へ短い本文を送る。
pub async fn send_test_email(
    settings: &HashMap<String, String>,
    to: &str,
) -> Result<(), MailError> {
    let (transport, from) = build_transport_with_timeout(settings, Some(TEST_TIMEOUT))?;

    let email = Message::builder()
        .from(from.parse()?)
        .to(to.trim().parse()?)
        .subject("seiran — テストメール")
        .header(ContentType::TEXT_PLAIN)
        .body("seiran の管理画面から送信したテストメールです。このメールが届いていれば、SMTP 設定でメールを送信できます。".to_string())?;

    transport.send(email).await?;
    Ok(())
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

/// TOTP（#65）: 認証アプリ・リカバリーコードを両方失ったユーザー向けの強制解除メール。
pub async fn send_totp_disable_email(
    settings: &HashMap<String, String>,
    to: &str,
    disable_url: &str,
) -> Result<(), MailError> {
    let (transport, from) = build_transport(settings)?;

    let body = format!(
        "seiran — 二段階認証（TOTP）の解除リクエストを受け付けました。\n\n以下のリンクをクリックすると、このアカウントの二段階認証を無効化できます:\n\n{}\n\nこのリンクは 1 時間有効です。\n\n心当たりがない場合はこのメールを無視してください（リンクを踏まない限り二段階認証は解除されません）。",
        disable_url
    );

    let email = Message::builder()
        .from(from.parse()?)
        .to(to.parse()?)
        .subject("seiran — 二段階認証の解除リクエスト")
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

/// `com.atproto.server.createSession`（ATPセッション作成）の`authFactorToken`確認コード。
pub async fn send_atp_session_2fa_code(
    settings: &HashMap<String, String>,
    to: &str,
    code: &str,
) -> Result<(), MailError> {
    let (transport, from) = build_transport(settings)?;

    let body = format!(
        "seiran — ログイン確認コードです。\n\nAT Protocolクライアント（Bluesky公式アプリ等）でのログインを完了するには、\n以下のコードを入力してください:\n\n{}\n\nこのコードは15分間有効です。\n\n心当たりがない場合はこのメールを無視してください。",
        code
    );

    let email = Message::builder()
        .from(from.parse()?)
        .to(to.parse()?)
        .subject("seiran — ログイン確認コード")
        .header(ContentType::TEXT_PLAIN)
        .body(body)?;

    transport.send(email).await?;
    Ok(())
}

/// `com.atproto.identity.requestPlcOperationSignature`のPLCオペレーション署名確認コード。
pub async fn send_plc_operation_signature_code(
    settings: &HashMap<String, String>,
    to: &str,
    code: &str,
) -> Result<(), MailError> {
    let (transport, from) = build_transport(settings)?;

    let body = format!(
        "seiran — DIDの管理情報（他サーバーへの引っ越し等）を変更するリクエストを受け付けました。\n\n以下のコードを、操作を行っているクライアントに入力してください:\n\n{}\n\nこのコードは15分間有効です。\n\n心当たりがない場合はこのメールを無視してください（コードを入力しない限りDIDの情報は変更されません）。",
        code
    );

    let email = Message::builder()
        .from(from.parse()?)
        .to(to.parse()?)
        .subject("seiran — DID管理情報の変更確認コード")
        .header(ContentType::TEXT_PLAIN)
        .body(body)?;

    transport.send(email).await?;
    Ok(())
}

/// DID 転入の入力待ちの種類ごとの、案内メールで伝える「次にやること」。
fn migration_input_wait_action(status: &str) -> &'static str {
    match status {
        "awaiting_user_confirmation" => {
            "確認コードの入力は済んでいます。転入画面で最終確認をしてください。転入すると元のサーバー（bsky.social 等）ではログインできなくなり、ハンドルが変わります。内容を確かめて「転入する」を押すと転入が確定します。"
        }
        "awaiting_plc_token" => {
            "転入元（Bluesky 等）から届いた確認コードを、転入画面に入力してください。コードは届いてから15分で切れますが、転入画面の「確認コードを送り直す」で取り直せます。"
        }
        "awaiting_source_password" => {
            "転入元にアプリパスワードでログインしていたため、転入に必要な操作ができませんでした。転入画面で、転入元にログインするときのパスワード（アプリパスワードではない方）を入力してください。"
        }
        _ => {
            "転入の途中でエラーが起き、止まっています。転入画面で、リトライするか、中断して別のアカウントや通常の新規登録でやり直すかを選んでください。"
        }
    }
}

/// DID 転入の案内メールで共通の、続きへ戻る方法。
fn migration_resume_instructions(username: &str, resume_url: &str) -> String {
    format!(
        "次のリンクから転入画面へ戻れます（一度だけ使えます）:\n\n{resume_url}\n\nリンクを使ったあとや別の端末からは、ログイン画面でユーザー名「{username}」と、転入を始めたときに決めたパスワードを入力しても戻れます。"
    )
}

fn format_deadline(deadline: chrono::DateTime<chrono::Utc>) -> String {
    deadline.format("%Y-%m-%d %H:%M UTC").to_string()
}

/// DID 転入の入力待ちの案内（入力待ちに入った時点で送る）。
pub async fn send_migration_input_wait_notice(
    settings: &HashMap<String, String>,
    to: &str,
    status: &str,
    username: &str,
    resume_url: &str,
    deadline: chrono::DateTime<chrono::Utc>,
) -> Result<(), MailError> {
    let (transport, from) = build_transport(settings)?;
    let body = format!(
        "seiran への Bluesky からの転入（ユーザー名「{username}」）は、あなたの入力を待っています。\n\n{action}\n\n{resume}\n\n{deadline} までに続きを進めないと、転入は取り消され、確保しているユーザー名「{username}」は解放されます。\n\n心当たりがない場合はこのメールを無視してください。",
        action = migration_input_wait_action(status),
        resume = migration_resume_instructions(username, resume_url),
        deadline = format_deadline(deadline),
    );
    let email = Message::builder()
        .from(from.parse()?)
        .to(to.parse()?)
        .subject("seiran — Bluesky からの転入: 入力をお待ちしています")
        .header(ContentType::TEXT_PLAIN)
        .body(body)?;
    transport.send(email).await?;
    Ok(())
}

/// DID 転入の打ち切りの警告（入力待ちのまま24時間たった時点で送る）。
pub async fn send_migration_expiry_warning(
    settings: &HashMap<String, String>,
    to: &str,
    status: &str,
    username: &str,
    resume_url: &str,
    deadline: chrono::DateTime<chrono::Utc>,
) -> Result<(), MailError> {
    let (transport, from) = build_transport(settings)?;
    let body = format!(
        "seiran への Bluesky からの転入（ユーザー名「{username}」）は、{deadline} に取り消されます。\n\n続ける場合は、それまでに次の操作をしてください。\n\n{action}\n\n{resume}\n\n取り消されると、確保しているユーザー名「{username}」は解放されます。転入をやめる場合は、このメールを無視してかまいません。",
        action = migration_input_wait_action(status),
        resume = migration_resume_instructions(username, resume_url),
        deadline = format_deadline(deadline),
    );
    let email = Message::builder()
        .from(from.parse()?)
        .to(to.parse()?)
        .subject("seiran — Bluesky からの転入はまもなく取り消されます")
        .header(ContentType::TEXT_PLAIN)
        .body(body)?;
    transport.send(email).await?;
    Ok(())
}

/// DID 転入を期限切れで取り消した通知。
pub async fn send_migration_expired(
    settings: &HashMap<String, String>,
    to: &str,
    username: &str,
) -> Result<(), MailError> {
    let (transport, from) = build_transport(settings)?;
    let body = format!(
        "seiran への Bluesky からの転入（ユーザー名「{username}」）は、入力が無いまま期限が過ぎたため取り消しました。確保していたユーザー名「{username}」は解放されています。\n\n転入元のアカウントは何も変わっていません。転入する場合は、もう一度最初から始めてください。"
    );
    let email = Message::builder()
        .from(from.parse()?)
        .to(to.parse()?)
        .subject("seiran — Bluesky からの転入を取り消しました")
        .header(ContentType::TEXT_PLAIN)
        .body(body)?;
    transport.send(email).await?;
    Ok(())
}

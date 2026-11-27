use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use seiran_common::edit_diff::Edit;

use crate::error::ApiError;
use crate::AppState;

// ─── レスポンス DTO ────────────────────────────────────────────────────────

/// smtp_password はレスポンスに含めず smtp_password_set: bool に置き換える。
#[derive(Serialize)]
pub struct SiteSettingsResponse {
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_username: String,
    pub smtp_password_set: bool,
    pub smtp_from: String,
    pub require_email_verification: String,
    // サイト外観（#30）
    pub site_name: String,
    pub site_color: String,
    pub site_icon_url: String,
    /// アップロード経由で設定した場合の対応 `media_files.sha256`（PWAアイコン配信用）。
    pub site_icon_sha256: String,
    /// アップロード経由で設定した場合の対応 `media_files.id`（文字列）。孤立ファイルGC
    /// （`maintenance::orphaned_media_files`）が参照有無を判定するために見る。
    pub site_icon_media_file_id: String,
    pub media_proxy_url: String,
    // ログイン画面デザイン（#243）
    /// サイト説明テキスト。HTML可・改行保持、サニタイズしない（管理者専用入力のため）。
    pub site_description: String,
    /// 利用規約テキスト。HTML可・改行保持、サニタイズしない（管理者専用入力のため）。
    /// 空文字列なら登録画面は同意チェックボックス自体を表示しない。
    pub terms_of_service_text: String,
    // 「このサイトについて」（`handlers::about`）。いずれも任意。
    /// 運営者名。
    pub operator_name: String,
    /// 運営者の URL（http/https のみ）。
    pub operator_url: String,
    /// 問い合わせ先メールアドレス。
    pub contact_email: String,
    /// ログイン画面背景メディアのURL（画像/動画どちらも可）。
    pub login_bg_url: String,
    /// "image" | "video"。login_bg_url のメディア種別。
    pub login_bg_type: String,
    /// アップロード経由で設定した場合の対応 `media_files.id`（文字列）。用途は
    /// `site_icon_media_file_id` と同じ。
    pub login_bg_media_file_id: String,
    // 認証ブルートフォース対策（#223）
    pub auth_bruteforce_window_minutes: String,
    pub auth_bruteforce_max_variants: String,
    pub auth_ip_block_window_minutes: String,
    pub auth_ip_block_threshold: String,
    pub auth_ip_block_duration_hours: String,
    // Cloudflare Turnstile（#223）: secret_key はレスポンスに含めず設定有無のみ返す
    pub turnstile_site_key: String,
    pub turnstile_secret_key_set: bool,
    // Cloudflare DNS（ハンドル検証 TXT）: api_token はレスポンスに含めず設定有無のみ返す
    pub cloudflare_api_token_set: bool,
    pub cloudflare_zone_id: String,
    // パスワードリセット/確認メールのレート制限（#223）
    pub password_reset_max_active: String,
    // アカウント生成のIPレート制限（#223）
    pub account_creation_ip_window_minutes: String,
    pub account_creation_ip_max: String,
    // ロール別レート制限（#223フォローアップ）
    pub post_rate_limit_window_minutes: String,
    pub post_rate_limit_max_user: String,
    pub post_rate_limit_max_moderator: String,
    pub follow_rate_limit_window_hours: String,
    pub follow_rate_limit_max_user: String,
    pub follow_rate_limit_max_moderator: String,
    pub follow_rate_limit_max_quarantine: String,
    pub list_max_count_user: String,
    pub list_max_count_moderator: String,
    pub list_member_max_user: String,
    pub list_member_max_moderator: String,
    pub scheduled_post_max_user: String,
    pub scheduled_post_max_moderator: String,
    pub search_rate_limit_window_minutes: String,
    pub search_rate_limit_max_user: String,
    pub search_rate_limit_max_moderator: String,
    // URLカード埋め込みプレーヤー（oEmbed discovery）の許可ドメイン（改行区切り、後方一致）
    pub oembed_allowed_domains: String,
    // ユーザー数上限（0 = 無制限）。退会済み・凍結済み・DID転出済みを含むアカウント総数が
    // これに達すると新規登録・転入を拒否する。
    pub max_user_accounts: String,
    /// "true"なら新規登録・転入の受付を停止する。
    pub registration_closed: String,
}

fn build_response(settings: &HashMap<String, String>) -> SiteSettingsResponse {
    SiteSettingsResponse {
        smtp_host: value(settings, "smtp_host"),
        smtp_port: value(settings, "smtp_port"),
        smtp_username: value(settings, "smtp_username"),
        smtp_password_set: !value(settings, "smtp_password").is_empty(),
        smtp_from: value(settings, "smtp_from"),
        require_email_verification: value(settings, "require_email_verification"),
        site_name: value(settings, "site_name"),
        site_color: value(settings, "site_color"),
        site_icon_url: value(settings, "site_icon_url"),
        site_icon_sha256: value(settings, "site_icon_sha256"),
        site_icon_media_file_id: value(settings, "site_icon_media_file_id"),
        media_proxy_url: value(settings, "media_proxy_url"),
        site_description: value(settings, "site_description"),
        terms_of_service_text: value(settings, "terms_of_service_text"),
        operator_name: value(settings, "operator_name"),
        operator_url: value(settings, "operator_url"),
        contact_email: value(settings, "contact_email"),
        login_bg_url: value(settings, "login_bg_url"),
        login_bg_type: value(settings, "login_bg_type"),
        login_bg_media_file_id: value(settings, "login_bg_media_file_id"),
        auth_bruteforce_window_minutes: value(settings, "auth_bruteforce_window_minutes"),
        auth_bruteforce_max_variants: value(settings, "auth_bruteforce_max_variants"),
        auth_ip_block_window_minutes: value(settings, "auth_ip_block_window_minutes"),
        auth_ip_block_threshold: value(settings, "auth_ip_block_threshold"),
        auth_ip_block_duration_hours: value(settings, "auth_ip_block_duration_hours"),
        turnstile_site_key: value(settings, "turnstile_site_key"),
        turnstile_secret_key_set: !value(settings, "turnstile_secret_key").is_empty(),
        cloudflare_api_token_set: !value(settings, "cloudflare_api_token").is_empty(),
        cloudflare_zone_id: value(settings, "cloudflare_zone_id"),
        password_reset_max_active: value(settings, "password_reset_max_active"),
        account_creation_ip_window_minutes: value(settings, "account_creation_ip_window_minutes"),
        account_creation_ip_max: value(settings, "account_creation_ip_max"),
        post_rate_limit_window_minutes: value(settings, "post_rate_limit_window_minutes"),
        post_rate_limit_max_user: value(settings, "post_rate_limit_max_user"),
        post_rate_limit_max_moderator: value(settings, "post_rate_limit_max_moderator"),
        follow_rate_limit_window_hours: value(settings, "follow_rate_limit_window_hours"),
        follow_rate_limit_max_user: value(settings, "follow_rate_limit_max_user"),
        follow_rate_limit_max_moderator: value(settings, "follow_rate_limit_max_moderator"),
        follow_rate_limit_max_quarantine: value(settings, "follow_rate_limit_max_quarantine"),
        list_max_count_user: value(settings, "list_max_count_user"),
        list_max_count_moderator: value(settings, "list_max_count_moderator"),
        list_member_max_user: value(settings, "list_member_max_user"),
        list_member_max_moderator: value(settings, "list_member_max_moderator"),
        scheduled_post_max_user: value(settings, "scheduled_post_max_user"),
        scheduled_post_max_moderator: value(settings, "scheduled_post_max_moderator"),
        search_rate_limit_window_minutes: value(settings, "search_rate_limit_window_minutes"),
        search_rate_limit_max_user: value(settings, "search_rate_limit_max_user"),
        search_rate_limit_max_moderator: value(settings, "search_rate_limit_max_moderator"),
        oembed_allowed_domains: value(settings, "oembed_allowed_domains"),
        max_user_accounts: value(settings, "max_user_accounts"),
        registration_closed: value(settings, "registration_closed"),
    }
}

// ─── 設定項目の表 ──────────────────────────────────────────────────────────

/// 差分の当て方（`seiran_common::edit_diff`）。
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// 現在値が編集前か編集後の値なら当てる。
    Value,
    /// 複数行のテキスト。3-way マージで当てる。
    Text,
    /// 書き込み専用（レスポンスでは設定の有無だけを返す）。画面は現在値を知らないので、
    /// 入力があれば上書きする。
    Secret,
}

/// 保存する値の検証。空文字列（未設定に戻す）は常に通す。
#[derive(Clone, Copy)]
enum Check {
    None,
    /// 正の整数。
    PositiveInt,
    /// 0 以上の整数（0 は無制限）。
    NonNegativeInt,
    /// http/https の URL。値はエラーコード。
    HttpUrl(&'static str),
    Email,
}

#[derive(Clone, Copy)]
enum Normalize {
    None,
    Trim,
    TrimTrailingSlash,
}

struct Field {
    key: &'static str,
    kind: Kind,
    /// 行が無いときの値。レスポンスにも差分の突き合わせにもこの値を使う。
    default: &'static str,
    check: Check,
    normalize: Normalize,
}

const FIELDS: &[Field] = &[
    Field {
        key: "smtp_host",
        kind: Kind::Value,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "smtp_port",
        kind: Kind::Value,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "smtp_username",
        kind: Kind::Value,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "smtp_password",
        kind: Kind::Secret,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "smtp_from",
        kind: Kind::Value,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "require_email_verification",
        kind: Kind::Value,
        default: "false",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "site_name",
        kind: Kind::Value,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "site_color",
        kind: Kind::Value,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "site_icon_url",
        kind: Kind::Value,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "site_icon_sha256",
        kind: Kind::Value,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "site_icon_media_file_id",
        kind: Kind::Value,
        default: "",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "media_proxy_url",
        kind: Kind::Value,
        default: "",
        check: Check::HttpUrl("INVALID_MEDIA_PROXY_URL"),
        normalize: Normalize::TrimTrailingSlash,
    },
    Field {
        key: "site_description",
        kind: Kind::Text,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "terms_of_service_text",
        kind: Kind::Text,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "operator_name",
        kind: Kind::Value,
        default: "",
        check: Check::None,
        normalize: Normalize::Trim,
    },
    Field {
        key: "operator_url",
        kind: Kind::Value,
        default: "",
        check: Check::HttpUrl("INVALID_OPERATOR_URL"),
        normalize: Normalize::Trim,
    },
    Field {
        key: "contact_email",
        kind: Kind::Value,
        default: "",
        check: Check::Email,
        normalize: Normalize::Trim,
    },
    Field {
        key: "login_bg_url",
        kind: Kind::Value,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "login_bg_type",
        kind: Kind::Value,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "login_bg_media_file_id",
        kind: Kind::Value,
        default: "",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "auth_bruteforce_window_minutes",
        kind: Kind::Value,
        default: "10",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "auth_bruteforce_max_variants",
        kind: Kind::Value,
        default: "5",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "auth_ip_block_window_minutes",
        kind: Kind::Value,
        default: "10",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "auth_ip_block_threshold",
        kind: Kind::Value,
        default: "5",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "auth_ip_block_duration_hours",
        kind: Kind::Value,
        default: "24",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "turnstile_site_key",
        kind: Kind::Value,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "turnstile_secret_key",
        kind: Kind::Secret,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "cloudflare_api_token",
        kind: Kind::Secret,
        default: "",
        check: Check::None,
        normalize: Normalize::Trim,
    },
    Field {
        key: "cloudflare_zone_id",
        kind: Kind::Value,
        default: "",
        check: Check::None,
        normalize: Normalize::Trim,
    },
    Field {
        key: "password_reset_max_active",
        kind: Kind::Value,
        default: "10",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "account_creation_ip_window_minutes",
        kind: Kind::Value,
        default: "60",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "account_creation_ip_max",
        kind: Kind::Value,
        default: "5",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "post_rate_limit_window_minutes",
        kind: Kind::Value,
        default: "60",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "post_rate_limit_max_user",
        kind: Kind::Value,
        default: "30",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "post_rate_limit_max_moderator",
        kind: Kind::Value,
        default: "100",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "follow_rate_limit_window_hours",
        kind: Kind::Value,
        default: "24",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "follow_rate_limit_max_user",
        kind: Kind::Value,
        default: "100",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "follow_rate_limit_max_moderator",
        kind: Kind::Value,
        default: "300",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "follow_rate_limit_max_quarantine",
        kind: Kind::Value,
        default: "10",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "list_max_count_user",
        kind: Kind::Value,
        default: "5",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "list_max_count_moderator",
        kind: Kind::Value,
        default: "30",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "list_member_max_user",
        kind: Kind::Value,
        default: "50",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "list_member_max_moderator",
        kind: Kind::Value,
        default: "300",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "scheduled_post_max_user",
        kind: Kind::Value,
        default: "10",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "scheduled_post_max_moderator",
        kind: Kind::Value,
        default: "30",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "search_rate_limit_window_minutes",
        kind: Kind::Value,
        default: "60",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "search_rate_limit_max_user",
        kind: Kind::Value,
        default: "10",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "search_rate_limit_max_moderator",
        kind: Kind::Value,
        default: "50",
        check: Check::PositiveInt,
        normalize: Normalize::None,
    },
    Field {
        key: "oembed_allowed_domains",
        kind: Kind::Text,
        default: "",
        check: Check::None,
        normalize: Normalize::None,
    },
    Field {
        key: "max_user_accounts",
        kind: Kind::Value,
        default: "100",
        check: Check::NonNegativeInt,
        normalize: Normalize::None,
    },
    Field {
        key: "registration_closed",
        kind: Kind::Value,
        default: "false",
        check: Check::None,
        normalize: Normalize::None,
    },
];

/// 設定の現在値（行が無ければ既定値）。
fn value(settings: &HashMap<String, String>, key: &str) -> String {
    settings.get(key).cloned().unwrap_or_else(|| {
        FIELDS
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.default.to_string())
            .unwrap_or_default()
    })
}

// ─── リクエスト DTO ────────────────────────────────────────────────────────

/// キーは `FIELDS` の `key`。値は差分 `{"from": 編集前, "to": 編集後}` か、編集後の値だけ
/// （上書き）。表に無いキーは無視する。
pub type UpdateSiteSettingsRequest = HashMap<String, Edit<String>>;

/// `mailto:` リンクにしても壊れない程度のメールアドレスか（`@` の前後が空でなく、空白や
/// `mailto:` の区切りになる文字を含まない）。厳密な検証はしない。
fn is_plausible_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '?' | '&' | '#' | '<' | '>' | '"' | '/'))
        && !domain.contains('@')
}

// ─── ハンドラ ─────────────────────────────────────────────────────────────

/// GET /api/admin/site-settings
pub async fn get_site_settings(
    State(state): State<AppState>,
) -> Result<Json<SiteSettingsResponse>, ApiError> {
    let settings = state
        .site_settings
        .get_all()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(build_response(&settings)))
}

fn normalize(field: &Field, v: String) -> String {
    match field.normalize {
        Normalize::None => v,
        Normalize::Trim => v.trim().to_string(),
        Normalize::TrimTrailingSlash => v.trim_end_matches('/').to_string(),
    }
}

fn check(field: &Field, v: &str) -> Result<(), ApiError> {
    if v.trim().is_empty() {
        return Ok(());
    }
    let invalid = || ApiError::BadRequest(format!("INVALID_{}", field.key.to_uppercase()));
    match field.check {
        Check::None => Ok(()),
        Check::PositiveInt => match v.parse::<i64>() {
            Ok(n) if n > 0 => Ok(()),
            _ => Err(invalid()),
        },
        Check::NonNegativeInt => match v.parse::<i64>() {
            Ok(n) if n >= 0 => Ok(()),
            _ => Err(invalid()),
        },
        // `/about` 等でリンクにするので、`javascript:` 等を入れさせない
        Check::HttpUrl(code) => match url::Url::parse(v) {
            Ok(url) if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() => {
                Ok(())
            }
            _ => Err(ApiError::BadRequest(code.to_string())),
        },
        Check::Email if is_plausible_email(v) => Ok(()),
        Check::Email => Err(ApiError::BadRequest("INVALID_CONTACT_EMAIL".to_string())),
    }
}

/// PATCH /api/admin/site-settings
///
/// 項目ごとの差分をロック中の現在値に当てて保存する（`seiran_common::edit_diff`）。どれか
/// 1つでも当たらなければ何も保存せず `EDIT_CONFLICT` を返す。
pub async fn update_site_settings(
    State(state): State<AppState>,
    Json(req): Json<UpdateSiteSettingsRequest>,
) -> Result<Json<SiteSettingsResponse>, ApiError> {
    let mut edits: Vec<(&'static Field, Edit<String>)> = Vec::new();
    for field in FIELDS {
        let Some(edit) = req.get(field.key) else {
            continue;
        };
        let edit = edit.clone().map_target(|v| normalize(field, v));
        check(field, edit.target())?;
        edits.push((field, edit));
    }

    let settings = state
        .site_settings
        .update_with(Box::new(move |current| {
            edits
                .into_iter()
                .map(|(field, edit)| {
                    let now = value(current, field.key);
                    let merged = match field.kind {
                        Kind::Value => edit.apply_value(&now)?,
                        Kind::Text => edit.apply_text(&now)?,
                        Kind::Secret => edit.target().clone(),
                    };
                    Ok((field.key.to_string(), merged))
                })
                .collect()
        }))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))??;

    Ok(Json(build_response(&settings)))
}

// ─── SMTP の接続確認・テスト送信 ───────────────────────────────────────────

/// 管理画面のフォームに入っている（保存前の）SMTP 設定。保存してからでないと試せないと、
/// 誤った設定を一度保存してメール確認・パスワードリセットを壊してしまうため、
/// 入力中の値で試せるようにする。省略した項目と空のパスワードは保存済みの値を使う。
#[derive(Deserialize, Default)]
pub struct SmtpTestRequest {
    pub smtp_host: Option<String>,
    pub smtp_port: Option<String>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub smtp_from: Option<String>,
    /// テスト送信の宛先。接続確認では使わない。
    pub to: Option<String>,
    /// テスト送信の言語（管理画面の表示言語）。無い・非対応なら `Accept-Language` から決める。
    pub language: Option<String>,
}

/// SMTP サーバーの応答はそのまま管理者に見せたいので、失敗も 200 で返し、
/// `code`（画面で翻訳する種別）と `detail`（lettre のエラー文・未設定の項目名）を載せる。
#[derive(Serialize)]
pub struct SmtpTestResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl SmtpTestResponse {
    fn from_result(result: Result<(), crate::mailer::MailError>) -> Self {
        use crate::mailer::MailError;
        match result {
            Ok(()) => Self {
                ok: true,
                code: None,
                detail: None,
            },
            Err(e) => {
                let (code, detail) = match e {
                    MailError::Config(field) => ("SMTP_NOT_CONFIGURED", field.to_string()),
                    MailError::Address(e) => ("SMTP_INVALID_ADDRESS", e.to_string()),
                    other
                    @ (MailError::Build(_) | MailError::Send(_) | MailError::NotConnected) => {
                        ("SMTP_FAILED", other.to_string())
                    }
                };
                Self {
                    ok: false,
                    code: Some(code),
                    detail: Some(detail),
                }
            }
        }
    }
}

/// 保存済みの設定に、リクエストで渡された SMTP 項目を重ねる。
fn merge_smtp_overrides(
    mut settings: HashMap<String, String>,
    req: &SmtpTestRequest,
) -> HashMap<String, String> {
    let password = req.smtp_password.as_ref().filter(|v| !v.is_empty());
    for (key, value) in [
        ("smtp_host", req.smtp_host.as_ref()),
        ("smtp_port", req.smtp_port.as_ref()),
        ("smtp_username", req.smtp_username.as_ref()),
        ("smtp_password", password),
        ("smtp_from", req.smtp_from.as_ref()),
    ] {
        if let Some(v) = value {
            settings.insert(key.to_string(), v.trim().to_string());
        }
    }
    settings
}

async fn smtp_settings_for_test(
    state: &AppState,
    req: &SmtpTestRequest,
) -> Result<HashMap<String, String>, ApiError> {
    let saved = state
        .site_settings
        .get_all()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(merge_smtp_overrides(saved, req))
}

/// POST /api/admin/site-settings/smtp-test
pub async fn test_smtp_connection(
    State(state): State<AppState>,
    Json(req): Json<SmtpTestRequest>,
) -> Result<Json<SmtpTestResponse>, ApiError> {
    let settings = smtp_settings_for_test(&state, &req).await?;
    let result = crate::mailer::test_smtp_connection(&settings).await;
    Ok(Json(SmtpTestResponse::from_result(result)))
}

/// POST /api/admin/site-settings/smtp-test-email
pub async fn send_smtp_test_email(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SmtpTestRequest>,
) -> Result<Json<SmtpTestResponse>, ApiError> {
    let to = req
        .to
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::BadRequest("SMTP_TEST_RECIPIENT_REQUIRED".to_string()))?
        .to_string();
    let settings = smtp_settings_for_test(&state, &req).await?;
    let language = crate::i18n::request_language(req.language.as_deref(), &headers);
    let result = crate::mailer::send_test_email(&settings, language, &to).await;
    Ok(Json(SmtpTestResponse::from_result(result)))
}

// ─── Turnstile の動作確認 ─────────────────────────────────────────────────

/// 管理画面のウィジェットで得たトークンと、入力中の（保存前の）Secret Key。鍵を誤って
/// 保存すると登録が `TURNSTILE_FAILED` で全滅するので、保存前に試せるようにする。
/// Secret Key を省略・空にすると保存済みの値を使う。
#[derive(Deserialize)]
pub struct TurnstileTestRequest {
    pub token: String,
    pub turnstile_secret_key: Option<String>,
}

/// SMTP の動作確認と同じく失敗も 200 で返す。`detail` は Siteverify の `error-codes`
/// （`invalid-input-secret` 等）か通信エラー文、`hostname` はウィジェットを解いたホスト名。
#[derive(Serialize)]
pub struct TurnstileTestResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

impl TurnstileTestResponse {
    fn failure(code: &'static str, detail: Option<String>) -> Self {
        Self {
            ok: false,
            code: Some(code),
            detail,
            hostname: None,
        }
    }
}

/// POST /api/admin/site-settings/turnstile-test
pub async fn test_turnstile(
    State(state): State<AppState>,
    ip: crate::middleware::ClientIp,
    Json(req): Json<TurnstileTestRequest>,
) -> Result<Json<TurnstileTestResponse>, ApiError> {
    let secret = match req
        .turnstile_secret_key
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        Some(v) => v.to_string(),
        None => state
            .site_settings
            .get("turnstile_secret_key")
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .unwrap_or_default()
            .trim()
            .to_string(),
    };
    if secret.is_empty() || req.token.trim().is_empty() {
        return Ok(Json(TurnstileTestResponse::failure(
            "TURNSTILE_NOT_CONFIGURED",
            None,
        )));
    }
    let response =
        match crate::handlers::auth::siteverify(&state, &secret, &req.token, ip.0.as_deref()).await
        {
            Ok(r) => r,
            Err(e) => {
                return Ok(Json(TurnstileTestResponse::failure(
                    "TURNSTILE_UNREACHABLE",
                    Some(e),
                )))
            }
        };
    if !response.success {
        let detail = (!response.error_codes.is_empty()).then(|| response.error_codes.join(", "));
        return Ok(Json(TurnstileTestResponse::failure(
            "TURNSTILE_FAILED",
            detail,
        )));
    }
    Ok(Json(TurnstileTestResponse {
        ok: true,
        code: None,
        detail: None,
        hostname: response.hostname,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plausible_email() {
        assert!(is_plausible_email("admin@example.com"));
        assert!(is_plausible_email("a.b+c@sub.example.jp"));
        for bad in [
            "admin",
            "@example.com",
            "admin@",
            "admin@localhost",
            "a b@example.com",
            "a@example.com?subject=x",
            "a@b@example.com",
            "a@.example.com",
        ] {
            assert!(!is_plausible_email(bad), "{bad}");
        }
    }

    #[test]
    fn overrides_replace_saved_values_but_empty_password_keeps_saved() {
        let saved = HashMap::from([
            ("smtp_host".to_string(), "old.example".to_string()),
            ("smtp_password".to_string(), "saved-secret".to_string()),
            ("smtp_from".to_string(), "a@example.com".to_string()),
        ]);
        let req = SmtpTestRequest {
            smtp_host: Some(" new.example ".to_string()),
            smtp_password: Some(String::new()),
            ..Default::default()
        };
        let merged = merge_smtp_overrides(saved, &req);
        assert_eq!(merged["smtp_host"], "new.example");
        assert_eq!(merged["smtp_password"], "saved-secret");
        assert_eq!(merged["smtp_from"], "a@example.com");
    }

    #[test]
    fn missing_host_reports_not_configured() {
        let res = SmtpTestResponse::from_result(Err(crate::mailer::MailError::Config("smtp_host")));
        assert!(!res.ok);
        assert_eq!(res.code, Some("SMTP_NOT_CONFIGURED"));
        assert_eq!(res.detail.as_deref(), Some("smtp_host"));
    }
}

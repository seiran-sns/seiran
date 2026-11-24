//! 「このサイトについて」（フロントの `/about`）に出す、サイト設定以外の情報。
//!
//! 利用規約・サイト名等は `POST /api/meta` に既にあるので、ここでは運営者・管理者一覧・
//! 問い合わせ先だけを返す。`/api/meta` はページを開くたびに呼ばれるため、管理者一覧の DB
//! 問い合わせをそちらに足さず、`/about` を開いたときだけ引く。

use std::collections::HashMap;

use axum::{extract::State, Json};
use serde::Serialize;

use crate::error::ApiError;
use crate::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutAdmin {
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    /// 表示名中の `:shortcode:` → 画像 URL。
    pub emojis: HashMap<String, String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutResponse {
    /// 運営者名（`site_settings.operator_name`）。未設定なら空文字列。
    pub operator_name: String,
    /// 運営者の URL（`site_settings.operator_url`、http/https のみ保存できる）。未設定なら空文字列。
    pub operator_url: String,
    /// ロールが `admin` のユーザー（未退会・未凍結）。
    pub admins: Vec<AboutAdmin>,
    /// 問い合わせ先メールアドレス（`site_settings.contact_email`）。未設定なら空文字列。
    pub contact_email: String,
}

/// `GET /api/about`（認証不要）
pub async fn about(State(state): State<AppState>) -> Result<Json<AboutResponse>, ApiError> {
    let rows = seiran_common::repository::actor::public_admins(&state.db)
        .await
        .map_err(|e| ApiError::Internal(format!("管理者一覧の取得に失敗: {e}")))?;
    let settings = state.site_settings.get_all().await.unwrap_or_default();
    let get = |k: &str| {
        settings
            .get(k)
            .map(|v| v.trim().to_string())
            .unwrap_or_default()
    };
    let admins = rows
        .into_iter()
        .map(|r| AboutAdmin {
            avatar_url: seiran_common::avatar::resolve_avatar_url(
                r.avatar_url,
                "local",
                &state.local_domain,
                r.actor_id,
            ),
            emojis: seiran_common::emoji_map::to_string_map(r.emoji_map.as_ref()),
            username: r.username,
            display_name: r.display_name,
        })
        .collect();
    Ok(Json(AboutResponse {
        operator_name: get("operator_name"),
        operator_url: get("operator_url"),
        admins,
        contact_email: get("contact_email"),
    }))
}

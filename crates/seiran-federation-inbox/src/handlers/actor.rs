use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;

use crate::AppState;

#[derive(Serialize)]
struct ApActorDocument {
    #[serde(rename = "@context")]
    context: Vec<serde_json::Value>,
    id: String,
    #[serde(rename = "type")]
    actor_type: String,
    #[serde(rename = "preferredUsername")]
    preferred_username: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    inbox: String,
    outbox: String,
    /// フォロー承認制（鍵アカウント、#依頼「プライバシー設定」）。`manuallyApprovesFollowers`
    /// はAP標準語彙（Mastodon/Misskey準拠）で、投稿の公開範囲には影響せずフォローの成立にのみ
    /// 本人の承認を要求する。
    #[serde(rename = "manuallyApprovesFollowers")]
    manually_approves_followers: bool,
    followers: String,
    following: String,
    /// ピン留め投稿（#61）。Mastodon 等はプロフィール表示時にこの URL を都度フェッチする。
    featured: String,
    /// 公開リスト一覧（#63、Mastodon にはない独自拡張）。
    lists: String,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<ApImage>,
    #[serde(rename = "publicKey")]
    public_key: ApPublicKey,
    /// プロフィールのキーバリュー項目（#62、Mastodon 等の「プロフィールのメタデータ欄」）。
    attachment: Vec<ApPropertyValue>,
    /// 表示名中のカスタム絵文字ショートコードをリモートが解決するための`Emoji`タグ（#186）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tag: Vec<serde_json::Value>,
    /// 生年月日（`birth_date_public=true`の場合のみ、Misskey互換の`vcard:bday`）。
    #[serde(rename = "vcard:bday", skip_serializing_if = "Option::is_none")]
    vcard_bday: Option<String>,
    /// プロフィールの「別のアカウント」（seiran独自拡張、`docs/protocols.md`参照）。
    /// 本来Move専用のフィールドだが、ここでは本人が自己申告した関連アカウント一覧を
    /// そのまま公開する（検証は読み手側の責務、Mastodon等と同じ「自己申告を公開する」流儀）。
    #[serde(rename = "alsoKnownAs", skip_serializing_if = "Vec::is_empty")]
    also_known_as: Vec<String>,
    /// リモートseiranアクターの相互申告マージ用の自己申告（seiran独自拡張、#236）。
    /// `actors.at_did`が未設定（ドメイン未確定期間等でDID未発行）の間は省略する。
    #[serde(rename = "seiranAtDid", skip_serializing_if = "Option::is_none")]
    seiran_at_did: Option<String>,
}

#[derive(Serialize)]
struct ApPropertyValue {
    #[serde(rename = "type")]
    kind: String,
    name: String,
    value: String,
}

#[derive(Serialize)]
struct ApImage {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "mediaType")]
    media_type: String,
    url: String,
}

#[derive(Serialize)]
struct ApPublicKey {
    id: String,
    owner: String,
    #[serde(rename = "publicKeyPem")]
    public_key_pem: String,
}

/// プロフィールのキーバリュー項目の値を PropertyValue 用 HTML にする（#62）。Mastodon 等の
/// クライアントは `value` を HTML としてレンダリングするため、単なるエスケープだけでは
/// URL がクリック可能なリンクにならない。`http(s)://` で始まる値は `<a>` タグでラップする
/// （Mastodon 自身が「サイト」等のフィールドに URL を入力した際に行うのと同じ変換）。
fn property_value_html(value: &str) -> String {
    let trimmed = value.trim();
    let escaped = trimmed
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        format!(
            r#"<a href="{0}" rel="me nofollow noopener noreferrer" target="_blank">{0}</a>"#,
            escaped
        )
    } else {
        escaped
    }
}

pub async fn actor_handler(
    Path(username): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let row = sqlx::query(
        "SELECT a.id, a.display_name, a.bio, \
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url, \
                mf.mime_type AS avatar_mime_type, a.profile_fields, a.emoji_map, \
                a.birth_date, a.birth_date_public, a.is_locked, a.at_did \
         FROM actors a \
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id \
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
         WHERE a.username = $1 AND a.actor_type = 'local' AND a.withdrawn_at IS NULL LIMIT 1",
    )
    .bind(&username)
    .fetch_optional(&state.db)
    .await;

    let (
        actor_id,
        display_name,
        bio,
        avatar_url,
        avatar_mime_type,
        profile_fields,
        emoji_map,
        birth_date,
        is_locked,
        seiran_at_did,
    ) = match row {
        Ok(Some(r)) => {
            let actor_id = r.try_get::<i64, _>("id").unwrap_or_default();
            let display_name = r
                .try_get::<Option<String>, _>("display_name")
                .ok()
                .flatten()
                .unwrap_or_else(|| username.clone());
            let bio = r.try_get::<Option<String>, _>("bio").ok().flatten();
            let stored_avatar_url = r.try_get::<Option<String>, _>("avatar_url").ok().flatten();
            let avatar_url = Some(stored_avatar_url.clone().unwrap_or_else(|| {
                seiran_common::avatar::fallback_avatar_url(&state.local_domain, actor_id)
            }));
            let avatar_mime_type = stored_avatar_url
                .as_ref()
                .and_then(|_| {
                    r.try_get::<Option<String>, _>("avatar_mime_type")
                        .ok()
                        .flatten()
                })
                .or_else(|| Some("image/svg+xml".to_string()));
            let profile_fields = r
                .try_get::<serde_json::Value, _>("profile_fields")
                .ok()
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();
            let emoji_map = r
                .try_get::<serde_json::Value, _>("emoji_map")
                .unwrap_or_else(|_| serde_json::json!({}));
            let birth_date_public: bool = r.try_get("birth_date_public").unwrap_or(false);
            let birth_date = if birth_date_public {
                r.try_get::<Option<chrono::NaiveDate>, _>("birth_date")
                    .unwrap_or(None)
            } else {
                None
            };
            let is_locked: bool = r.try_get("is_locked").unwrap_or(false);
            let seiran_at_did: Option<String> = r.try_get("at_did").ok().flatten();
            (
                actor_id,
                display_name,
                bio,
                avatar_url,
                avatar_mime_type,
                profile_fields,
                emoji_map,
                birth_date,
                is_locked,
                seiran_at_did,
            )
        }
        Ok(None) => return (StatusCode::NOT_FOUND, "").into_response(),
        Err(e) => {
            tracing::error!("[Actor] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let mut tag = Vec::new();
    seiran_common::ap::deliver::append_emoji_tags(
        &display_name,
        &emoji_map,
        &mut tag,
        &state.local_domain,
    );

    let attachment: Vec<ApPropertyValue> = profile_fields
        .iter()
        .filter_map(|f| {
            let name = f.get("name")?.as_str()?.to_string();
            let value = f.get("value")?.as_str()?;
            Some(ApPropertyValue {
                kind: "PropertyValue".to_string(),
                name,
                value: property_value_html(value),
            })
        })
        .collect();

    let base = format!("https://{}", state.local_domain);
    let actor_uri = format!("{}/users/{}", base, username);

    // プロフィールの「別のアカウント」（alsoKnownAs、seiran独自拡張）。自己申告をそのまま
    // 公開する（検証は読み手側の責務）。ターゲットの種別に応じてURI形式を作り分ける:
    // ローカルは自ドメインのactor URI、fediは保存済みのap_uri、bskyはdid:...形式
    // （Bridgy Fedと同じ流儀、`docs/protocols.md`参照）。
    let also_known_as_rows = sqlx::query(
        "SELECT a.actor_type::text AS actor_type, a.username, a.ap_uri, a.at_did
         FROM actor_also_known_as aka
         JOIN actors a ON a.id = aka.target_actor_id
         WHERE aka.owner_actor_id = $1",
    )
    .bind(actor_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let also_known_as: Vec<String> = also_known_as_rows
        .iter()
        .filter_map(|r| {
            let actor_type: String = r.try_get("actor_type").ok()?;
            match actor_type.as_str() {
                "local" => {
                    let target_username: String = r.try_get("username").ok()?;
                    Some(format!("{}/users/{}", base, target_username))
                }
                "bsky" => r.try_get::<Option<String>, _>("at_did").ok().flatten(),
                _ => r.try_get::<Option<String>, _>("ap_uri").ok().flatten(),
            }
        })
        .collect();

    let icon = avatar_url.map(|url| ApImage {
        kind: "Image".to_string(),
        media_type: avatar_mime_type.unwrap_or_else(|| "image/jpeg".to_string()),
        url,
    });

    let mut context = vec![
        serde_json::json!("https://www.w3.org/ns/activitystreams"),
        serde_json::json!("https://w3id.org/security/v1"),
    ];
    if birth_date.is_some() {
        context.push(serde_json::json!({"vcard": "http://www.w3.org/2006/vcard/ns#"}));
    }

    let doc = ApActorDocument {
        context,
        id: actor_uri.clone(),
        actor_type: "Person".to_string(),
        preferred_username: username.clone(),
        name: display_name,
        summary: bio,
        inbox: format!("{}/inbox", base),
        outbox: format!("{}/users/{}/outbox", base, username),
        manually_approves_followers: is_locked,
        followers: format!("{}/users/{}/followers", base, username),
        following: format!("{}/users/{}/following", base, username),
        featured: format!("{}/users/{}/collections/featured", base, username),
        lists: format!("{}/users/{}/lists", base, username),
        url: format!("{}/@{}", base, username),
        icon,
        public_key: ApPublicKey {
            id: format!("{}#main-key", actor_uri),
            owner: actor_uri,
            public_key_pem: state.ap_public_key_pem.clone(),
        },
        attachment,
        tag,
        vcard_bday: birth_date.map(|d| d.format("%Y-%m-%d").to_string()),
        also_known_as,
        seiran_at_did,
    };

    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/activity+json",
        )],
        Json(doc),
    )
        .into_response()
}

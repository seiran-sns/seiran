//! 公開カスタム絵文字一覧（Misskey 互換: `GET /api/emojis`）。
//!
//! Misskey クライアントはリアクションピッカー描画のため、ログイン前から未認証でこの
//! エンドポイントを呼ぶ。管理用の CRUD（`/api/admin/emojis`、要 admin 認証）とは別に、
//! 閲覧専用のエンドポイントとして公開する。レスポンス形状は Misskey の `EmojisResponse`
//! （`id`/`aliases`/`name`/`category`/`host`/`url`）に合わせる。

use axum::{extract::State, response::IntoResponse, Json};
use serde::Serialize;
use sqlx::Row;

use crate::AppState;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PublicEmoji {
    pub id: String,
    pub aliases: Vec<String>,
    pub name: String,
    pub category: Option<String>,
    /// ローカル絵文字は常に `null`（Misskey 準拠。リモートインスタンス由来の絵文字と区別する
    /// フィールドだが、seiran は現状ローカル絵文字のみを持つ）。
    pub host: Option<String>,
    pub url: String,
    pub license: Option<String>,
}

#[derive(Serialize)]
pub struct EmojisResponse {
    pub emojis: Vec<PublicEmoji>,
}

/// カスタム絵文字一覧を Misskey 互換の形状で取得する。`/api/emojis` と `/api/meta` の
/// `emojis` フィールドの両方から共有される。
pub async fn fetch_public_emojis(db: &sqlx::PgPool) -> Vec<PublicEmoji> {
    let rows = sqlx::query(
        "SELECT ce.id, ce.shortcode, ce.category, ce.tags, ce.license,
                rtrim(sp.public_url, '/') || '/' || mf.storage_key AS url
         FROM custom_emojis ce
         JOIN media_files mf ON mf.id = ce.media_file_id
         JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         ORDER BY ce.id",
    )
    .fetch_all(db)
    .await
    .unwrap_or_default();

    rows.into_iter()
        .filter_map(|row| {
            let id: i64 = row.try_get("id").ok()?;
            let shortcode: String = row.try_get("shortcode").ok()?;
            let url: String = row.try_get("url").ok()?;
            Some(PublicEmoji {
                id: id.to_string(),
                aliases: row.try_get("tags").unwrap_or_default(),
                name: shortcode,
                category: row.try_get::<Option<String>, _>("category").unwrap_or(None),
                host: None,
                url,
                license: row.try_get::<Option<String>, _>("license").unwrap_or(None),
            })
        })
        .collect()
}

/// GET /api/emojis
pub async fn list_emojis(State(state): State<AppState>) -> impl IntoResponse {
    let emojis = fetch_public_emojis(&state.db).await;
    Json(EmojisResponse { emojis })
}

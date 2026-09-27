//! 公開カスタム絵文字一覧（Misskey 互換: `GET /api/emojis`）。
//!
//! Misskey クライアントはリアクションピッカー描画のため、ログイン前から未認証でこの
//! エンドポイントを呼ぶ。管理用の CRUD（`/api/admin/emojis`、要 admin 認証）とは別に、
//! 閲覧専用のエンドポイントとして公開する。レスポンス形状は Misskey の `EmojisResponse`
//! （`id`/`aliases`/`name`/`category`/`host`/`url`）に合わせる。

use axum::{extract::State, response::IntoResponse, Json};
use serde::Serialize;

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
    /// 画像の実寸（`media_files`由来）。ピッカーのアスペクト比に応じたグリッド配置に使う。
    /// Misskey互換形状への追加フィールドなので、知らないクライアントは無視すればよい。
    pub width: i32,
    pub height: i32,
    /// 画像フェッチ完了までのプレースホルダ用（`media_files.blurhash`）。
    pub blurhash: String,
}

#[derive(Serialize)]
pub struct EmojisResponse {
    pub emojis: Vec<PublicEmoji>,
}

/// カスタム絵文字一覧を Misskey 互換の形状で取得する。`/api/emojis` と `/api/meta` の
/// `emojis` フィールドの両方から共有される。
pub async fn fetch_public_emojis(db: &sqlx::PgPool) -> Vec<PublicEmoji> {
    seiran_common::repository::emoji::list_public(db)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            // 画像の寸法と blurhash が無い絵文字はクライアントが扱えないので出さない。
            let (Some(width), Some(height), Some(blurhash)) = (row.width, row.height, row.blurhash)
            else {
                return None;
            };
            Some(PublicEmoji {
                id: row.id.to_string(),
                aliases: row.tags,
                name: row.shortcode,
                category: row.category,
                host: None,
                url: row.url,
                license: row.license,
                width,
                height,
                blurhash,
            })
        })
        .collect()
}

/// GET /api/emojis
pub async fn list_emojis(State(state): State<AppState>) -> impl IntoResponse {
    let emojis = fetch_public_emojis(&state.db).await;
    Json(EmojisResponse { emojis })
}

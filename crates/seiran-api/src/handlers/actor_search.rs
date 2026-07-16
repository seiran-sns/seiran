//! アクターのサジェスト検索（リスト機能のメンバー追加入力補助等）。
//!
//! DB（`actors`テーブル）に既に存在するアクターのみを対象にした軽量な部分一致検索。
//! WebFinger解決等の新規リモート取得は行わない（あくまで「知っている」アクターの絞り込み）。

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;

use crate::middleware::AuthedUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct ActorSearchQuery {
    pub q: String,
    pub limit: Option<i64>,
}

/// `GET /api/actors/search?q=...&limit=...`
/// ユーザー名・表示名の部分一致でDB上のアクターを検索する（リスト機能のメンバー追加
/// サジェスト用）。list-relay等の`users`行を持たないシステムアクターは除外する。
pub async fn search_actors(
    _user: AuthedUser,
    Query(q): Query<ActorSearchQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // `username`/`domain`はDBでは別カラムのため、"@yuba@reax.work"や"yuba@"のように
    // "@"を含む入力は素の`username ILIKE`単体では一致しない。先頭の"@"を除去した上で、
    // `username || '@' || domain`の結合文字列も検索対象に加えて acct 形式の入力に対応する。
    let query = q.q.trim().trim_start_matches('@');
    if query.is_empty() {
        return Json(Vec::<serde_json::Value>::new()).into_response();
    }
    let limit = q.limit.unwrap_or(10).clamp(1, 30);
    let contains_pattern = format!("%{}%", escape_like(query));
    let prefix_pattern = format!("{}%", escape_like(query));

    let rows = sqlx::query_as::<_, (i64, String, String, Option<String>, String, Option<String>)>(
        "SELECT a.id, a.username, a.domain, a.display_name, a.actor_type::text AS actor_type,
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url
         FROM actors a
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         WHERE (a.actor_type != 'local' OR a.user_id IS NOT NULL)
           AND (
             a.username ILIKE $1 ESCAPE '\\'
             OR a.display_name ILIKE $1 ESCAPE '\\'
             OR (a.username || '@' || a.domain) ILIKE $1 ESCAPE '\\'
           )
         ORDER BY (CASE WHEN a.username ILIKE $2 ESCAPE '\\' THEN 0 ELSE 1 END), a.username
         LIMIT $3",
    )
    .bind(&contains_pattern)
    .bind(&prefix_pattern)
    .bind(limit)
    .fetch_all(&state.db)
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[actor_search] DBエラー: {}", e);
            return Json(Vec::<serde_json::Value>::new()).into_response();
        }
    };

    let out: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(id, username, domain, display_name, actor_type, avatar_url)| {
            // add_member にそのまま渡せるターゲット文字列を計算する。
            let target = match actor_type.as_str() {
                "local" => username.clone(),
                "bsky" => username.clone(), // ハンドル（domainは空文字のため username がハンドル本体）
                _ => format!("{}@{}", username, domain),
            };
            serde_json::json!({
                "actor_id": id.to_string(),
                "username": username,
                "domain": domain,
                "display_name": display_name,
                "actor_type": actor_type,
                "avatar_url": avatar_url,
                "target": target,
            })
        })
        .collect();

    Json(out).into_response()
}

/// `ILIKE` パターン中の `%`/`_`/`\` をエスケープする（ユーザー入力をそのままワイルドカードに
/// しないため）。
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

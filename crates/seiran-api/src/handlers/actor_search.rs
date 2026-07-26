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

type ActorSearchRow = (i64, String, String, Option<String>, String, Option<String>);

/// `GET /api/actors/search?q=...&limit=...`
/// ユーザー名・表示名の部分一致でDB上のアクターを検索する（リスト機能のメンバー追加
/// サジェスト用）。list-relay等の`users`行を持たないシステムアクターは除外する。
pub async fn search_actors(
    _user: AuthedUser,
    Query(q): Query<ActorSearchQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // リスト編集・DM宛先向け。先頭の`@`を含む生入力を、表示名・Fedi/Bsky/ローカルの
    // 各ハンドルを改行で区切って連結した検索用文字列へ部分一致させる。
    let query = q.q.trim();
    if query.is_empty() {
        return Json(Vec::<serde_json::Value>::new()).into_response();
    }
    let limit = q.limit.unwrap_or(10).clamp(1, 30);
    let contains_pattern = format!("%{}%", escape_like(query));

    let rows = sqlx::query_as::<_, ActorSearchRow>(
        "SELECT a.id, a.username, a.domain, a.display_name, a.actor_type::text AS actor_type,
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url
         FROM actors a
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         WHERE (a.actor_type != 'local' OR a.user_id IS NOT NULL)
           AND LOWER(
             COALESCE(a.display_name, '')
             || E'\\n@' || a.username
             || CASE WHEN a.domain <> '' THEN '@' || a.domain ELSE '' END
             || CASE WHEN a.actor_type = 'local'
                     THEN E'\\n@' || a.username || '.' || a.domain ELSE '' END
           ) LIKE LOWER($1) ESCAPE '\\'
         ORDER BY a.username
         LIMIT $2",
    )
    .bind(&contains_pattern)
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
        .map(
            |(id, username, domain, display_name, actor_type, avatar_url)| {
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
            },
        )
        .collect();

    Json(out).into_response()
}

/// `GET /api/actors/suggest?q=...&limit=...`
/// 投稿欄の`@`入力補助向け。`q`は先頭の`@`を除いた文字列で、ハンドルに前方一致させる。
pub async fn suggest_actors(
    _user: AuthedUser,
    Query(q): Query<ActorSearchQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let query = q.q.trim().trim_start_matches('@');
    if query.is_empty() {
        return Json(Vec::<serde_json::Value>::new()).into_response();
    }
    let limit = q.limit.unwrap_or(10).clamp(1, 30);
    let pattern = format!("{}%", escape_like(query));
    let local_bsky_pattern = format!("\n@{}%", escape_like(query));

    let rows = sqlx::query_as::<_, ActorSearchRow>(
        "SELECT a.id, a.username, a.domain, a.display_name, a.actor_type::text AS actor_type,
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url
         FROM actors a
         JOIN (
           (SELECT id FROM actors
            WHERE LOWER(username || CASE WHEN domain <> '' THEN '@' || domain ELSE '' END)
                    LIKE LOWER($1) ESCAPE '\\'
            LIMIT $3)
           UNION
           (SELECT id FROM actors
            WHERE LOWER(CASE WHEN actor_type = 'local'
                             THEN E'\\n@' || username || '.' || domain END)
                    LIKE LOWER($2) ESCAPE '\\'
            LIMIT $3)
         ) candidate ON candidate.id = a.id
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         WHERE (a.actor_type != 'local' OR a.user_id IS NOT NULL)
         ORDER BY a.username
         LIMIT $3",
    )
    .bind(&pattern)
    .bind(&local_bsky_pattern)
    .bind(limit)
    .fetch_all(&state.db)
    .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("[actor_suggest] DBエラー: {}", e);
            return Json(Vec::<serde_json::Value>::new()).into_response();
        }
    };

    let query_lower = query.to_lowercase();
    let out: Vec<serde_json::Value> = rows
        .into_iter()
        .map(
            |(id, username, domain, display_name, actor_type, avatar_url)| {
                let target = suggestion_target(&actor_type, &username, &domain, &query_lower);
                serde_json::json!({
                    "actor_id": id.to_string(),
                    "username": username,
                    "domain": domain,
                    "display_name": display_name,
                    "actor_type": actor_type,
                    "avatar_url": avatar_url,
                    "target": target,
                })
            },
        )
        .collect();

    Json(out).into_response()
}

fn suggestion_target(actor_type: &str, username: &str, domain: &str, query_lower: &str) -> String {
    if actor_type != "local" {
        return if domain.is_empty() {
            username.to_owned()
        } else {
            format!("{}@{}", username, domain)
        };
    }

    let username_lower = username.to_lowercase();
    if username_lower.starts_with(query_lower) {
        username.to_owned()
    } else if format!("{}@", username_lower).starts_with(query_lower) {
        format!("{}@{}", username, domain)
    } else {
        format!("{}.{}", username_lower, domain)
    }
}

/// `ILIKE` パターン中の `%`/`_`/`\` をエスケープする（ユーザー入力をそのままワイルドカードに
/// しないため）。
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::suggestion_target;

    #[test]
    fn local_target_follows_matching_handle_form() {
        assert_eq!(
            suggestion_target("local", "Alice", "example.com", "ali"),
            "Alice"
        );
        assert_eq!(
            suggestion_target("local", "Alice", "example.com", "alice@"),
            "Alice@example.com"
        );
        assert_eq!(
            suggestion_target("local", "Alice", "example.com", "alice."),
            "alice.example.com"
        );
    }
}

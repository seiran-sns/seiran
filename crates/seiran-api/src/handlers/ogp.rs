//! ポスト詳細・プロフィールページの OGP (Open Graph) 対応。
//!
//! フロントエンドは SPA のため、素の index.html には投稿・プロフィールごとの
//! `<meta>` タグが無い。かといって User-Agent で bot を判定して出し分ける方式だと、
//! リストにない未知のクローラーを取りこぼす。そのため `/notes/:id`・`/@:handle` への
//! リクエスト（AP クライアント向けを除く）は常に、SPA の index.html
//! （`state.frontend_origin` から取得）の `<head>` に OGP `<meta>` を注入して返す。
//! 実ブラウザはそのまま SPA が起動し、クローラーは JS を実行しないため
//! `<meta>` だけを読んで終わる。bot 判定・リダイレクトは不要。

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
};
use sqlx::Row;

use crate::handlers::notes::{fetch_attachments_map, to_note_response};
use crate::AppState;

const DESCRIPTION_MAX_GRAPHEMES: usize = 140;

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 本文中の内部リンクマーカー `[表示テキスト](URL)`（`docs/protocols.md` 6節）を
/// 表示テキストのみに展開する。OGP description はプレーンテキストとして見せたいため、
/// URL部分は不要。
fn strip_link_markers(body: &str) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut result = String::with_capacity(body.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' {
            if let Some(close_bracket_offset) = chars[i + 1..].iter().position(|&c| c == ']') {
                let text_end = i + 1 + close_bracket_offset;
                if chars.get(text_end + 1) == Some(&'(') {
                    if let Some(close_paren_offset) = chars[text_end + 2..].iter().position(|&c| c == ')') {
                        let url_end = text_end + 2 + close_paren_offset;
                        result.extend(&chars[i + 1..text_end]);
                        i = url_end + 1;
                        continue;
                    }
                }
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

/// 書記素クラスタ単位で切り詰め、末尾が切れていれば `…` を付ける。
fn truncate_graphemes(s: &str, max: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let graphemes: Vec<&str> = s.graphemes(true).collect();
    if graphemes.len() <= max {
        s.to_string()
    } else {
        format!("{}…", graphemes[..max].concat())
    }
}

/// SPA の `<head>` に注入する OGP/Twitter Card の `<meta>` 群を組み立てる。
fn build_meta_tags(title: &str, description: &str, image: Option<&str>, page_url: &str, site_name: &str) -> String {
    let title = escape_html(title);
    let description = escape_html(description);
    let page_url = escape_html(page_url);
    let site_name = escape_html(site_name);
    let image_tag = match image {
        Some(url) => format!(
            r#"<meta property="og:image" content="{}">
    <meta name="twitter:card" content="summary_large_image">"#,
            escape_html(url)
        ),
        None => r#"<meta name="twitter:card" content="summary">"#.to_string(),
    };

    format!(
        r#"<meta property="og:type" content="article">
    <meta property="og:site_name" content="{site_name}">
    <meta property="og:title" content="{title}">
    <meta property="og:description" content="{description}">
    <meta property="og:url" content="{page_url}">
    {image_tag}"#
    )
}

/// `<title>...</title>` の中身だけを置き換える（無ければ何もしない）。OGP 未対応の
/// 簡素なクローラーは `og:title` ではなくこちらを見ることがあるため、`<meta>` 注入と
/// 合わせて書き換える。
fn replace_title(html: &str, title: &str) -> String {
    let (Some(start), Some(end)) = (html.find("<title>"), html.find("</title>")) else {
        return html.to_string();
    };
    if start >= end {
        return html.to_string();
    }
    let content_start = start + "<title>".len();
    format!("{}{}{}", &html[..content_start], escape_html(title), &html[end..])
}

/// `state.frontend_origin` から SPA の index.html を取得する。フロントエンドがどのパスに
/// 対しても同じ index.html を返す（SPA fallback）前提のため、常にルート `/` を取得する
/// （`/notes`・`/@` に対する Vite の proxy 設定と衝突させないため）。
async fn fetch_spa_html(state: &AppState) -> Result<String, Response> {
    let resp = state
        .http_client
        .get(format!("{}/", state.frontend_origin))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("[ogp] フロントエンド取得失敗: {}", e);
            StatusCode::BAD_GATEWAY.into_response()
        })?;
    resp.text().await.map_err(|e| {
        tracing::error!("[ogp] フロントエンドレスポンス読み取り失敗: {}", e);
        StatusCode::BAD_GATEWAY.into_response()
    })
}

/// SPA の index.html を取得し、`<title>` を差し替えたうえで `</head>` の直前に
/// `meta_tags` を差し込んで返す。
async fn render_spa_with_ogp(title: &str, meta_tags: &str, state: &AppState) -> Response {
    let html = match fetch_spa_html(state).await {
        Ok(h) => h,
        Err(resp) => return resp,
    };
    let html = replace_title(&html, title);
    let injected = match html.find("</head>") {
        Some(idx) => format!("{}{}\n{}", &html[..idx], meta_tags, &html[idx..]),
        None => html,
    };
    (StatusCode::OK, Html(injected)).into_response()
}

/// 投稿/アクターが見つからない・DBエラー時のフォールバック。OGP `<meta>` は付けず、
/// SPA の index.html をそのまま返す（フロント側の「見つかりません」表示・エラー
/// ハンドリングに委ねる。ここで 404 等を返すと SPA 自体が起動できなくなってしまう）。
async fn render_spa_plain(state: &AppState) -> Response {
    match fetch_spa_html(state).await {
        Ok(html) => (StatusCode::OK, Html(html)).into_response(),
        Err(resp) => resp,
    }
}

/// Accept ヘッダーが ActivityPub 用（`application/activity+json` / `application/ld+json`）
/// を明示的に要求していないかどうか。true の場合は OGP 注入済み SPA HTML を返してよい
/// （AP クライアントには従来通り JSON-LD を返す必要がある）。
pub fn wants_html(headers: &HeaderMap) -> bool {
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    !accept.contains("application/activity+json") && !accept.contains("application/ld+json")
}

async fn site_name(state: &AppState) -> String {
    let settings = state.site_settings.get_all().await.unwrap_or_default();
    let name = settings.get("site_name").cloned().unwrap_or_default();
    if name.is_empty() { "seiran".to_string() } else { name }
}

/// ポスト詳細ページ用の OGP 付き SPA HTML を返す（`GET /notes/:id`、AP Accept 以外）。
pub async fn note_ogp_html(post_id: i64, state: &AppState) -> Response {
    let post = match state.posts.find_by_id_for_viewer(post_id, None).await {
        Ok(Some(p)) => p,
        Ok(None) => return render_spa_plain(state).await,
        Err(e) => {
            tracing::error!("[note_ogp_html] DB エラー: {}", e);
            return render_spa_plain(state).await;
        }
    };

    let mut att_map = fetch_attachments_map(&state.db, &[post_id]).await;
    let attachments = att_map.remove(&post_id).unwrap_or_default();
    let note = to_note_response(post, attachments);

    let display_name = note.user.display_name.clone().unwrap_or_else(|| note.user.username.clone());
    let handle = match &note.user.domain {
        Some(domain) if !domain.is_empty() => format!("@{}@{}", note.user.username, domain),
        _ => format!("@{}", note.user.username),
    };
    let title = format!("{}（{}）の投稿", display_name, handle);
    let flattened = strip_link_markers(&note.text).replace('\n', " ");
    let description = truncate_graphemes(&flattened, DESCRIPTION_MAX_GRAPHEMES);

    let image = note.attachments.iter().find_map(|a| {
        if a.mime_type.starts_with("image/") {
            Some(a.url.clone())
        } else {
            a.thumbnail_url.clone()
        }
    });

    let page_url = format!("https://{}/notes/{}", state.local_domain, note.id);
    let name = site_name(state).await;
    let meta_tags = build_meta_tags(&title, &description, image.as_deref(), &page_url, &name);
    render_spa_with_ogp(&title, &meta_tags, state).await
}

/// プロフィールページ用の OGP 付き SPA HTML を返す（`GET /@:handle`、AP Accept 以外）。
/// `handle` は `username`（ローカル）または `username@domain`（既知のリモートアクター）。
/// DB 未登録のリモートアクターは OGP 用の情報が引けないだけで SPA 自体は通常通り返す
/// （プロフィール表示自体は SPA 側が別途 AppView/AP から取得して表示する）。
pub async fn profile_ogp(Path(handle): Path<String>, State(state): State<AppState>) -> Response {
    let (username, domain) = match handle.split_once('@') {
        Some((u, d)) => (u.to_string(), d.to_string()),
        None => (handle.clone(), state.local_domain.clone()),
    };

    let row = sqlx::query(
        "SELECT a.display_name, a.bio, \
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url \
         FROM actors a \
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id \
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
         WHERE a.username = $1 AND a.domain = $2 LIMIT 1",
    )
    .bind(&username)
    .bind(&domain)
    .fetch_optional(&state.db)
    .await;

    let (display_name, bio, avatar_url) = match row {
        Ok(Some(r)) => {
            let display_name: Option<String> = r.try_get("display_name").ok().flatten();
            let bio: Option<String> = r.try_get("bio").ok().flatten();
            let avatar_url: Option<String> = r.try_get("avatar_url").ok().flatten();
            (display_name.unwrap_or_else(|| username.clone()), bio.unwrap_or_default(), avatar_url)
        }
        Ok(None) => return render_spa_plain(&state).await,
        Err(e) => {
            tracing::error!("[profile_ogp] DB エラー: {}", e);
            return render_spa_plain(&state).await;
        }
    };

    let is_local = domain == state.local_domain;
    let acct = if is_local { format!("@{}", username) } else { format!("@{}@{}", username, domain) };
    let title = format!("{}（{}）", display_name, acct);
    let description = truncate_graphemes(&bio, DESCRIPTION_MAX_GRAPHEMES);
    let page_url = format!("https://{}/@{}", state.local_domain, acct.trim_start_matches('@'));
    let name = site_name(&state).await;
    let meta_tags = build_meta_tags(&title, &description, avatar_url.as_deref(), &page_url, &name);
    render_spa_with_ogp(&title, &meta_tags, &state).await
}

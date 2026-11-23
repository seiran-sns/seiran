//! ポスト詳細・プロフィールページの OGP (Open Graph) 対応。
//!
//! フロントエンドは SPA のため、素の index.html には投稿・プロフィールごとの
//! `<meta>` タグが無い。かといって User-Agent で bot を判定して出し分ける方式だと、
//! リストにない未知のクローラーを取りこぼす。そのため `/notes/:id`・`/@:handle` への
//! リクエスト（AP クライアント向けを除く）は常に、SPA の index.html
//! （`state.frontend_origin` から取得）の `<head>` に OGP `<meta>` を注入して返す。
//! 実ブラウザはそのまま SPA が起動し、クローラーは JS を実行しないため
//! `<meta>` だけを読んで終わる。bot 判定・リダイレクトは不要。
//!
//! それ以外の SPA のページ（トップページ等）には、サイト名・サイト説明・サイトアイコンからなる
//! サイト全体の OGP を注入する（[`site_ogp_html`]、`handlers::frontend` から呼ばれる）。
//!
//! タイトルの書式と、サイト説明が未設定のときの既定文は `Accept-Language` の言語で出す
//! （`locales/<lang>/ogp.json`）。リンクカードを作るクローラーの多くは `Accept-Language` を
//! 送らないので、その場合は英語になる。管理者が書いたサイト説明・投稿本文は書かれた言語のまま。

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
};

use crate::handlers::notes::validation::strip_html_tags;
use crate::handlers::notes::{fetch_attachments_map, to_note_response};
use crate::i18n::{self, Namespace};
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
                    if let Some(close_paren_offset) =
                        chars[text_end + 2..].iter().position(|&c| c == ')')
                    {
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

/// OGP の `og:type`。
#[derive(Debug, Clone, Copy)]
enum OgType {
    /// サイト全体（トップページ等）。
    Website,
    /// 投稿・プロフィール。
    Article,
}

/// Twitter Card の種類。正方形のアイコン・アバターを横長の大きいカードに出すと上下が
/// 切れるので、大きいカードは投稿の添付画像だけに使う。
#[derive(Debug, Clone, Copy)]
enum CardSize {
    Summary,
    LargeImage,
}

/// SPA の `<head>` に注入する OGP の内容。
struct OgpMeta<'a> {
    og_type: OgType,
    title: &'a str,
    description: &'a str,
    image: Option<&'a str>,
    card: CardSize,
    page_url: &'a str,
    site_name: &'a str,
}

/// SPA の `<head>` に注入する OGP/Twitter Card の `<meta>` 群を組み立てる。
fn build_meta_tags(meta: &OgpMeta<'_>) -> String {
    let og_type = match meta.og_type {
        OgType::Website => "website",
        OgType::Article => "article",
    };
    let title = escape_html(meta.title);
    let description = escape_html(meta.description);
    let page_url = escape_html(meta.page_url);
    let site_name = escape_html(meta.site_name);
    let image_tag = match (meta.image, meta.card) {
        (Some(url), CardSize::LargeImage) => format!(
            r#"<meta property="og:image" content="{}">
    <meta name="twitter:card" content="summary_large_image">"#,
            escape_html(url)
        ),
        (Some(url), CardSize::Summary) => format!(
            r#"<meta property="og:image" content="{}">
    <meta name="twitter:card" content="summary">"#,
            escape_html(url)
        ),
        (None, _) => r#"<meta name="twitter:card" content="summary">"#.to_string(),
    };

    format!(
        r#"<meta name="description" content="{description}">
    <meta property="og:type" content="{og_type}">
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
    format!(
        "{}{}{}",
        &html[..content_start],
        escape_html(title),
        &html[end..]
    )
}

/// `state.frontend_origin` から SPA の index.html を取得する（`state.frontend_dir` があれば
/// そこから読む）。フロントエンドがどのパスに
/// 対しても同じ index.html を返す（SPA fallback）前提のため、常にルート `/` を取得する
/// （`/notes`・`/@` に対する Vite の proxy 設定と衝突させないため）。
/// `frontend_origin`は運用者が設定する内部ホスト（Docker の`frontend`等、private IP）のため、
/// 内部IPを拒否する連合用の`state.http_client`（`seiran_common::net::federation_client_builder`）
/// ではなく、この専用クライアントで取得する。
fn internal_http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("内部通信用HTTPクライアントの構築に失敗")
    })
}

async fn fetch_spa_html(state: &AppState) -> Result<String, Response> {
    // 自分でフロントエンドを配っている（公開イメージ）ならファイルを直接読む
    if let Some(dir) = state.frontend_dir.as_deref() {
        return super::frontend::read_index_html(dir).await.map_err(|e| {
            tracing::error!("[ogp] フロントエンドの index.html の読み出し失敗: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        });
    }
    let resp = internal_http_client()
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
    let mut response = (StatusCode::OK, Html(injected)).into_response();
    let headers = response.headers_mut();
    // タイトルの書式・既定の説明文が `Accept-Language` で変わる
    headers.insert(header::VARY, HeaderValue::from_static("accept-language"));
    // 設定・投稿の変更をすぐ反映させる（静的な index.html と同じ扱い）
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
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

/// OGP に使うサイト設定。
struct SiteInfo {
    /// サイト名（未設定なら `seiran`）。site_name はHTML可（#243）だが、OGP は HTML 想定で
    /// ないため、タグを除去したプレーンテキストにする（呼び出し側で更に `escape_html` される）。
    name: String,
    /// サイト説明（タグ除去・改行を空白に・切り詰め済み）。未設定なら `language` の既定文。
    description: String,
    /// サイトアイコンの絶対 URL。未設定なら `None`。
    icon_url: Option<String>,
}

/// カード画像に使うサイトアイコンの大きさ。`/favicon.ico` の 32px では小さすぎるため、
/// PWA マニフェストと同じ 512px 版を使う（`handlers::site_icon`）。
const SITE_ICON_OGP_SIZE: u32 = 512;

async fn site_info(state: &AppState, language: &str) -> SiteInfo {
    let settings = state.site_settings.get_all().await.unwrap_or_default();
    let get = |k: &str| settings.get(k).map(|v| v.trim()).unwrap_or_default();

    let name = match get("site_name") {
        "" => "seiran".to_string(),
        n => strip_html_tags(n),
    };
    let description = match get("site_description") {
        "" => i18n::text(Namespace::Ogp, language, "siteDescription", &[]),
        d => truncate_graphemes(
            &flatten_whitespace(&strip_html_tags(d)),
            DESCRIPTION_MAX_GRAPHEMES,
        ),
    };
    // アップロードしたアイコン（`site_icon_sha256`）を優先し、無ければ URL 指定のアイコン
    // （`handlers::favicon` と同じ順）。クローラーは相対 URL を解決しないことがあるので絶対にする。
    let icon_url = match (get("site_icon_sha256"), get("site_icon_url")) {
        ("", "") => None,
        ("", url) if url.starts_with('/') => Some(format!("https://{}{url}", state.local_domain)),
        ("", url) => Some(url.to_string()),
        (sha256, _) => Some(format!(
            "https://{}/api/site-icon/{sha256}/{SITE_ICON_OGP_SIZE}",
            state.local_domain
        )),
    };
    SiteInfo {
        name,
        description,
        icon_url,
    }
}

/// 改行・連続する空白を1つの空白にまとめる（description は1行のテキストとして見せる）。
fn flatten_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// サイト全体の OGP を注入した SPA HTML（トップページ等、`handlers::frontend` から呼ばれる）。
/// 投稿・プロフィールが見つからないときもこれを返す（404 を返すと SPA が起動できず、
/// 「見つかりません」表示やリモートアクターの取得が動かないため、SPA は常に返す）。
pub async fn site_ogp_html(state: &AppState, uri: &Uri, headers: &HeaderMap) -> Response {
    let language = i18n::request_language(None, headers);
    let page_url = format!("https://{}{}", state.local_domain, uri.path());
    site_ogp_for(state, language, &page_url).await
}

async fn site_ogp_for(state: &AppState, language: &str, page_url: &str) -> Response {
    let site = site_info(state, language).await;
    let meta_tags = build_meta_tags(&OgpMeta {
        og_type: OgType::Website,
        title: &site.name,
        description: &site.description,
        image: site.icon_url.as_deref(),
        card: CardSize::Summary,
        page_url,
        site_name: &site.name,
    });
    render_spa_with_ogp(&site.name, &meta_tags, state).await
}

/// ポスト詳細ページ用の OGP 付き SPA HTML を返す（`GET /notes/:id`、AP Accept 以外）。
pub async fn note_ogp_html(post_id: i64, headers: &HeaderMap, state: &AppState) -> Response {
    let language = i18n::request_language(None, headers);
    let page_url = format!("https://{}/notes/{}", state.local_domain, post_id);
    let post = match state.posts.find_by_id_for_viewer(post_id, None).await {
        Ok(Some(p)) => p,
        Ok(None) => return site_ogp_for(state, language, &page_url).await,
        Err(e) => {
            tracing::error!("[note_ogp_html] DB エラー: {}", e);
            return site_ogp_for(state, language, &page_url).await;
        }
    };

    let mut att_map = fetch_attachments_map(&state.db, &[post_id]).await;
    let attachments = att_map.remove(&post_id).unwrap_or_default();
    let note = to_note_response(post, attachments, vec![]);

    let display_name = note
        .user
        .display_name
        .clone()
        .unwrap_or_else(|| note.user.username.clone());
    let handle = seiran_common::username::actor_handle(
        &note.user.username,
        note.user.domain.as_deref().unwrap_or(""),
        &note.user.actor_type,
    );
    let title = i18n::text(
        Namespace::Ogp,
        language,
        "noteTitle",
        &[("name", &display_name), ("handle", &handle)],
    );
    let flattened = strip_link_markers(&note.text).replace('\n', " ");
    let description = truncate_graphemes(&flattened, DESCRIPTION_MAX_GRAPHEMES);

    let image = note.attachments.iter().find_map(|a| {
        if a.mime_type.starts_with("image/") {
            Some(a.url.clone())
        } else {
            a.thumbnail_url.clone()
        }
    });

    let site = site_info(state, language).await;
    let meta_tags = build_meta_tags(&OgpMeta {
        og_type: OgType::Article,
        title: &title,
        description: &description,
        image: image.as_deref(),
        card: CardSize::LargeImage,
        page_url: &page_url,
        site_name: &site.name,
    });
    render_spa_with_ogp(&title, &meta_tags, state).await
}

/// プロフィールページ用の OGP 付き SPA HTML を返す（`GET /@:handle`、AP Accept 以外）。
/// `handle` は `username`（ローカル）または `username@domain`（既知のリモートアクター）。
/// DB 未登録のリモートアクターは OGP 用の情報が引けないだけで SPA 自体は通常通り返す
/// （プロフィール表示自体は SPA 側が別途 AppView/AP から取得して表示する）。
pub async fn profile_ogp(
    Path(handle): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    let language = i18n::request_language(None, &headers);
    let (username, domain) = match handle.split_once('@') {
        Some((u, d)) => (u.to_string(), d.to_string()),
        None => (handle.clone(), state.local_domain.to_string()),
    };

    let row = seiran_common::repository::actor::ogp_profile(&state.db, &username, &domain).await;

    let (actor_type, display_name, bio, avatar_url) = match row {
        Ok(Some(r)) => {
            let seiran_common::repository::actor::OgpActorRow {
                actor_id,
                actor_type,
                display_name,
                bio,
                avatar_url,
            } = r;
            let avatar_url = seiran_common::avatar::resolve_avatar_url(
                avatar_url,
                &actor_type,
                &domain,
                actor_id,
            );
            (
                actor_type,
                display_name.unwrap_or_else(|| username.clone()),
                bio.unwrap_or_default(),
                avatar_url,
            )
        }
        Ok(None) => {
            let page_url = format!("https://{}/@{}", state.local_domain, handle);
            return site_ogp_for(&state, language, &page_url).await;
        }
        Err(e) => {
            tracing::error!("[profile_ogp] DB エラー: {}", e);
            let page_url = format!("https://{}/@{}", state.local_domain, handle);
            return site_ogp_for(&state, language, &page_url).await;
        }
    };

    let acct = seiran_common::username::actor_handle(&username, &domain, &actor_type);
    let title = i18n::text(
        Namespace::Ogp,
        language,
        "profileTitle",
        &[("name", &display_name), ("handle", &acct)],
    );
    let description = truncate_graphemes(&bio, DESCRIPTION_MAX_GRAPHEMES);
    let page_url = format!(
        "https://{}/@{}",
        state.local_domain,
        acct.trim_start_matches('@')
    );
    let site = site_info(&state, language).await;
    let meta_tags = build_meta_tags(&OgpMeta {
        og_type: OgType::Article,
        title: &title,
        description: &description,
        image: avatar_url.as_deref(),
        card: CardSize::Summary,
        page_url: &page_url,
        site_name: &site.name,
    });
    render_spa_with_ogp(&title, &meta_tags, &state).await
}

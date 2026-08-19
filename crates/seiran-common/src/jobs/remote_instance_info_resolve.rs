//! リモートインスタンス（Fedi、`actors.domain`単位）のnodeinfoを取得し
//! `remote_instance_meta` へキャッシュする（#NoteCardリモートサーバー表示）。
//!
//! notes API / Misskey互換API がノート一覧を組み立てる際、キャッシュ未登録のドメインを
//! 見つけるとこのジョブを積む（`AppState::enqueue_remote_instance_info_resolve`）。
//! 取得できてもできなくても必ず1行キャッシュする（未対応サーバーへ毎回リトライしないため）。
//! `theme_color` はリモートの宣言値を優先し、未宣言なら既知フォーク固有色→汎用デフォルト
//! （薄いグレー）の順にフォールバックした「表示に使う最終値」を書き込む（フロントエンドは
//! この値をそのまま描画するだけでよい設計、Misskey API `UserLite.instance.themeColor` 上位互換）。

use std::sync::Arc;

use regex::Regex;
use serde::Deserialize;

use crate::net::{fetch_validated_with_accept, FetchError};
use crate::queue::worker::JobContext;
use crate::repository::{PgRemoteInstanceMetaRepository, RemoteInstanceMetaRepository};

/// 未宣言・非対応サーバー共通の汎用デフォルト（薄いグレー）。
pub const DEFAULT_THEME_COLOR: &str = "#e4e4e7";

const ACCEPT_JSON: &[&str] = &["application/json", "application/ld+json"];
const ACCEPT_HTML: &[&str] = &["text/html", "application/xhtml+xml"];
const ACCEPT_IMAGE: &[&str] = &["image/", "application/octet-stream"];

#[derive(Deserialize)]
struct NodeinfoDiscovery {
    links: Vec<NodeinfoLink>,
}

#[derive(Deserialize)]
struct NodeinfoLink {
    rel: String,
    href: String,
}

#[derive(Deserialize)]
struct NodeinfoDoc {
    #[serde(default)]
    software: Option<NodeinfoSoftware>,
    #[serde(default)]
    metadata: Option<NodeinfoMetadata>,
}

#[derive(Deserialize)]
struct NodeinfoSoftware {
    name: Option<String>,
}

#[derive(Deserialize)]
struct NodeinfoMetadata {
    #[serde(rename = "nodeName")]
    node_name: Option<String>,
    #[serde(rename = "themeColor")]
    theme_color: Option<String>,
}

/// 既知の後発フォーク: nodeinfo で `themeColor` を宣言しない実装でも、ブランドカラーが
/// 広く知られているものだけ代替色を当てる。
fn fallback_color_for_software(software_name: &str) -> Option<&'static str> {
    match software_name.to_ascii_lowercase().as_str() {
        "fedibird" => Some("#f4dced"), // 薄い赤紫
        "kmyblue" => Some("#d9ecfa"),  // 薄いブルー
        "mitra" => Some("#e8d9a6"),    // 濃いめのクリーム色
        "akkoma" => Some("#ddd9f5"),   // 薄い青紫
        _ => None,
    }
}

/// `#rgb`/`#rrggbb` のみ許容する（不正値をそのままフロントへ渡してCSSを壊さないため）。
fn is_valid_hex_color(s: &str) -> bool {
    let Some(hex) = s.strip_prefix('#') else {
        return false;
    };
    (hex.len() == 3 || hex.len() == 6) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// `<link rel="icon">`/`<link rel="shortcut icon">`のhref（属性順序は問わない）を抽出する。
fn extract_favicon_link(html: &str) -> Option<String> {
    let patterns = [
        r#"(?i)<link[^>]*?rel=["'](?:shortcut icon|icon)["'][^>]*?href=["']([^"']+)["']"#,
        r#"(?i)<link[^>]*?href=["']([^"']+)["'][^>]*?rel=["'](?:shortcut icon|icon)["']"#,
    ];
    for pattern in patterns {
        if let Ok(re) = Regex::new(pattern) {
            if let Some(cap) = re.captures(html) {
                let raw = cap.get(1)?.as_str();
                return Some(html_escape::decode_html_entities(raw).into_owned());
            }
        }
    }
    None
}

/// サーバーアイコンURLを解決する。まず`<link rel="icon">`をトップページHTMLから探し、
/// 無ければ`/favicon.ico`を実際に取得できるか試す（存在しないURLをそのまま返すと
/// フロントで壊れた画像アイコンになるため）。どちらも失敗すれば`None`。
async fn fetch_favicon_url(domain: &str) -> Option<String> {
    let home_url = format!("https://{domain}/");
    if let Ok((bytes, _)) = fetch_validated_with_accept(&home_url, ACCEPT_HTML, "text/html").await
    {
        let html = String::from_utf8_lossy(&bytes);
        if let Some(href) = extract_favicon_link(&html) {
            if let Ok(resolved) = reqwest::Url::parse(&home_url).and_then(|base| base.join(&href))
            {
                return Some(resolved.to_string());
            }
        }
    }

    let fallback = format!("https://{domain}/favicon.ico");
    fetch_validated_with_accept(&fallback, ACCEPT_IMAGE, "image/*")
        .await
        .ok()
        .map(|_| fallback)
}

fn pick_nodeinfo_link(links: &[NodeinfoLink]) -> Option<&str> {
    links
        .iter()
        .find(|l| l.rel.ends_with("schema/2.1"))
        .or_else(|| links.iter().find(|l| l.rel.ends_with("schema/2.0")))
        .or_else(|| links.iter().find(|l| l.rel.contains("nodeinfo")))
        .map(|l| l.href.as_str())
}

pub async fn handle(domain: String, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!(
            "[RemoteInstanceInfoResolve] DB pool 未設定のためスキップ (domain={})",
            domain
        );
        return Ok(());
    };
    let repo = PgRemoteInstanceMetaRepository::new(pool.clone());

    let result = fetch_nodeinfo(&domain).await;
    let (software_name, node_name, declared_theme_color) = match result {
        Ok(v) => v,
        Err(FetchError::FetchFailed | FetchError::UpstreamError | FetchError::DnsFailed) => {
            // 一時的な失敗の可能性があるためリトライさせる（キャッシュ行はまだ書かない）。
            return Err(format!(
                "nodeinfo取得失敗（リトライ対象）: domain={domain}"
            ));
        }
        Err(e) => {
            // nodeinfo未対応・不正レスポンス等は再試行しても無駄。汎用デフォルトで
            // 1行キャッシュし、以後このドメインへは二度と問い合わせない。
            tracing::info!(
                "[RemoteInstanceInfoResolve] 取得を諦めます domain={} reason={}",
                domain,
                e
            );
            (None, None, None)
        }
    };

    let theme_color = declared_theme_color
        .filter(|c| is_valid_hex_color(c))
        .or_else(|| software_name.as_deref().and_then(fallback_color_for_software).map(str::to_string))
        .unwrap_or_else(|| DEFAULT_THEME_COLOR.to_string());
    let icon_url = fetch_favicon_url(&domain).await;

    repo.upsert(
        &domain,
        software_name.as_deref(),
        node_name.as_deref(),
        Some(&theme_color),
        icon_url.as_deref(),
    )
    .await
    .map_err(|e| format!("remote_instance_meta UPSERT失敗: {}", e))?;

    tracing::info!(
        "[RemoteInstanceInfoResolve] キャッシュ完了 domain={} software={:?}",
        domain,
        software_name
    );
    Ok(())
}

#[allow(clippy::type_complexity)]
async fn fetch_nodeinfo(
    domain: &str,
) -> Result<(Option<String>, Option<String>, Option<String>), FetchError> {
    let discovery_url = format!("https://{domain}/.well-known/nodeinfo");
    let (bytes, _) = fetch_validated_with_accept(&discovery_url, ACCEPT_JSON, "application/json")
        .await?;
    let discovery: NodeinfoDiscovery =
        serde_json::from_slice(&bytes).map_err(|_| FetchError::UnsupportedType)?;
    let href = pick_nodeinfo_link(&discovery.links).ok_or(FetchError::UnsupportedType)?;

    let (doc_bytes, _) = fetch_validated_with_accept(href, ACCEPT_JSON, "application/json").await?;
    let doc: NodeinfoDoc =
        serde_json::from_slice(&doc_bytes).map_err(|_| FetchError::UnsupportedType)?;

    let software_name = doc.software.and_then(|s| s.name);
    let (node_name, theme_color) = match doc.metadata {
        Some(m) => (m.node_name, m.theme_color),
        None => (None, None),
    };
    Ok((software_name, node_name, theme_color))
}

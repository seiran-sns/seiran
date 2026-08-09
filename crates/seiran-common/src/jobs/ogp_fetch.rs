//! Fedi受信投稿の本文中URL（YouTube/Spotify/x.com以外の一般URL）のOGPを取得し
//! `post_link_cards` へ保存する。取得できなければ静かに諦める（投稿自体は既に保存済みのため、
//! そのURLがカード無しのまま表示されるだけで実害は小さい）。

use std::sync::Arc;

use regex::Regex;

use crate::net::{fetch_validated_with_accept, FetchError};
use crate::queue::worker::JobContext;

/// `<meta property="..." content="...">`（属性順序は問わない）から`content`を抽出する。
/// HTML5準拠の厳密なパースはせず、OGPメタタグの一般的な形だけを対象にした簡易実装。
fn extract_og_content(html: &str, property: &str) -> Option<String> {
    let escaped = regex::escape(property);
    let patterns = [
        format!(r#"<meta[^>]*?property=["']{escaped}["'][^>]*?content=["']([^"']*)["']"#),
        format!(r#"<meta[^>]*?content=["']([^"']*)["'][^>]*?property=["']{escaped}["']"#),
    ];
    for pattern in patterns {
        if let Ok(re) = Regex::new(&pattern) {
            if let Some(cap) = re.captures(html) {
                let raw = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                return Some(html_escape::decode_html_entities(raw).into_owned());
            }
        }
    }
    None
}

pub async fn handle(
    post_id: i64,
    url: String,
    position: i16,
    ctx: Arc<JobContext>,
) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!("[OgpFetch] DB pool 未設定のためスキップ (post_id={})", post_id);
        return Ok(());
    };

    let (bytes, _content_type) = match fetch_validated_with_accept(
        &url,
        &["text/html", "application/xhtml+xml"],
        "text/html,application/xhtml+xml;q=0.9",
    )
    .await
    {
        Ok(v) => v,
        Err(FetchError::FetchFailed | FetchError::UpstreamError | FetchError::DnsFailed) => {
            // 一時的な失敗の可能性があるためリトライさせる。
            return Err(format!("OGP取得失敗（リトライ対象）: url={url}"));
        }
        Err(e) => {
            // 不正なURL・プライベートアドレス・非対応Content-Type等は再試行しても無駄なので諦める。
            tracing::info!("[OgpFetch] 取得を諦めます url={} reason={}", url, e);
            return Ok(());
        }
    };

    let html = String::from_utf8_lossy(&bytes);
    let Some(title) = extract_og_content(&html, "og:title") else {
        tracing::info!("[OgpFetch] og:titleが見つからないため諦めます url={}", url);
        return Ok(());
    };
    let description = extract_og_content(&html, "og:description").unwrap_or_default();
    let thumbnail_url = extract_og_content(&html, "og:image").and_then(|raw| {
        // 相対URLで書かれているサイトもあるため、対象ページのURLを基点に絶対URL化する。
        reqwest::Url::parse(&url)
            .ok()
            .and_then(|base| base.join(&raw).ok())
            .map(|u| u.to_string())
    });

    sqlx::query(
        "INSERT INTO post_link_cards (post_id, position, url, title, description, thumbnail_url)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(post_id)
    .bind(position)
    .bind(&url)
    .bind(&title)
    .bind(&description)
    .bind(&thumbnail_url)
    .execute(pool)
    .await
    .map_err(|e| format!("post_link_cards INSERT失敗: {}", e))?;

    tracing::info!("[OgpFetch] 保存完了 post_id={} url={}", post_id, url);
    Ok(())
}

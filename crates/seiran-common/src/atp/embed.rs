//! Bsky embed（`record.embed`）から添付・URLカード・引用先を復元する共通ロジック。
//!
//! Jetstream 経由の通常投稿取り込み（`seiran-atp-repo`）と、AppView からの単発投稿取得
//! （`fetch_single_bsky_post`/`upsert_bsky_post`。リポスト取り込み・検索結果保存・
//! ピン留め投稿同期・「開く」機能で使われる）の両方で必要になるため、ここに共通実装を置く。

use serde_json::Value as JsonValue;

/// Bsky embed（画像・動画）から復元した添付情報。CDN URL は DID + blob CID のみから
/// 決定的に組み立てられる（Bluesky AppView への追加問い合わせは不要）。
#[derive(Debug, Clone)]
pub struct ParsedAttachment {
    pub url: String,
    pub mime_type: String,
    pub width: i32,
    pub height: i32,
    pub thumbnail_url: Option<String>,
    /// GIFアニメ由来（Tenor/Klipy GIFピッカー、または`app.bsky.embed.video`の
    /// `presentation:"gif"`＝GIFファイル直接アップロード）。フロントで自動再生・
    /// ミュート・ループ・コントロール無し表示に切り替えるためのフラグ。
    pub is_gif: bool,
}

/// `embed.external.thumb`（blob参照）から Bsky CDN のサムネイル URL を組み立てる。
/// GIF/動画添付・URLカードの双方から共有される。
pub fn bsky_external_thumb_url(embed: &JsonValue, did: &str) -> Option<String> {
    embed
        .get("external")
        .and_then(|external| external.get("thumb"))
        .and_then(|thumb| thumb.get("ref"))
        .and_then(|reference| reference.get("$link"))
        .and_then(|cid| cid.as_str())
        .map(|cid| {
            format!(
                "https://cdn.bsky.app/img/feed_thumbnail/plain/{}/{}",
                urlencoding::encode(did),
                cid
            )
        })
}

pub fn bsky_gif_video_attachment(
    uri: &str,
    embed: &JsonValue,
    did: &str,
) -> Option<ParsedAttachment> {
    let (base, query) = uri.split_once('?')?;
    let dimensions = query
        .split('&')
        .filter_map(|part| part.split_once('='))
        .fold(
            (None, None, None, None),
            |(height, width, mp4, webm), (key, value)| match key {
                "hh" => (value.parse::<i32>().ok(), width, mp4, webm),
                "ww" => (height, value.parse::<i32>().ok(), mp4, webm),
                "mp4" => (height, width, Some(value), webm),
                "webm" => (height, width, mp4, Some(value)),
                _ => (height, width, mp4, webm),
            },
        );
    let (height, width, mp4_slug, webm_slug) = dimensions;
    let height = height.filter(|value| *value > 0)?;
    let width = width.filter(|value| *value > 0)?;

    let (url, mime_type) = if let Some(path) = base.strip_prefix("https://static.klipy.com/ii/") {
        let slug = mp4_slug.or(webm_slug)?;
        let extension = if mp4_slug.is_some() { "mp4" } else { "webm" };
        let (directory, _) = path.rsplit_once('/')?;
        (
            format!("https://k.gifs.bsky.app/ii/{directory}/{slug}.{extension}"),
            format!("video/{extension}"),
        )
    } else {
        let path = base.strip_prefix("https://media.tenor.com/")?;
        let (id, filename) = path.split_once('/')?;
        if !id.contains("AAAAC") || !filename.ends_with(".gif") {
            return None;
        }
        (
            format!(
                "https://t.gifs.bsky.app/{}/{}",
                id.replace("AAAAC", "AAAP1"),
                filename
                    .strip_suffix(".gif")
                    .unwrap_or(filename)
                    .to_string()
                    + ".mp4"
            ),
            "video/mp4".to_string(),
        )
    };

    let thumbnail_url = bsky_external_thumb_url(embed, did);

    Some(ParsedAttachment {
        url,
        mime_type,
        width,
        height,
        thumbnail_url,
        is_gif: true,
    })
}

/// Bsky embed の URL カード（`app.bsky.embed.external`）から復元したメタデータ。
/// GIF ピッカー由来（Tenor/Klipy、`bsky_gif_video_attachment` が動画化する）は除く。
#[derive(Debug, Clone)]
pub struct ParsedLinkCard {
    pub url: String,
    pub title: String,
    pub description: String,
    pub thumbnail_url: Option<String>,
}

/// `record.embed` が `app.bsky.embed.external` かつ GIF ピッカー由来でない場合のみ、
/// URL カード（YouTube/Spotify/x.com/一般）として表示するためのメタデータを返す。
/// `recordWithMedia` は AT Protocol の制約上 `external` を内包できないため対象外。
pub fn parse_bsky_embed_link_card(embed: &JsonValue, did: &str) -> Option<ParsedLinkCard> {
    let embed_type = embed.get("$type").and_then(|v| v.as_str()).unwrap_or("");
    if embed_type != "app.bsky.embed.external" {
        return None;
    }
    let external = embed.get("external")?;
    let uri = external.get("uri").and_then(|v| v.as_str())?;
    if bsky_gif_video_attachment(uri, embed, did).is_some() {
        return None;
    }
    let title = external
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let description = external
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let thumbnail_url = bsky_external_thumb_url(embed, did);
    Some(ParsedLinkCard {
        url: uri.to_string(),
        title,
        description,
        thumbnail_url,
    })
}

/// AP Note の `record.embed` を解析し、添付URL一覧を組み立てる。
/// `app.bsky.embed.images` → `https://cdn.bsky.app/img/feed_fullsize/plain/{did}/{cid}`
/// `app.bsky.embed.video` → HLSプレイリスト `https://video.bsky.app/watch/{did}/{cid}/playlist.m3u8`
///   （動画本体はBluesky公式の動画処理パイプラインでHLSにトランスコードされて配信されるため、
///   PDS上のblob自体を指すURLではなくこの固定パターンを使う。サムネイルも同様のパターン）。
///   `presentation:"gif"`付きはGIFファイル直接アップロード由来として`is_gif=true`にする。
/// `app.bsky.embed.recordWithMedia`（引用+メディア）は `media` フィールドを再帰的に見る。
/// `app.bsky.embed.external` は Bluesky の GIF ピッカーが生成する Tenor/Klipy URL のみ動画化する。
/// 未知の embed 種別や画像/動画以外（`record` 単体等）は空を返す。
pub fn parse_bsky_embed_attachments(embed: &JsonValue, did: &str) -> Vec<ParsedAttachment> {
    let embed_type = embed.get("$type").and_then(|v| v.as_str()).unwrap_or("");
    match embed_type {
        "app.bsky.embed.images" => embed
            .get("images")
            .and_then(|v| v.as_array())
            .map(|images| {
                images
                    .iter()
                    .filter_map(|img| {
                        let cid = img.get("image")?.get("ref")?.get("$link")?.as_str()?;
                        let mime_type = img
                            .get("image")
                            .and_then(|i| i.get("mimeType"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("image/jpeg")
                            .to_string();
                        let width = img
                            .get("aspectRatio")
                            .and_then(|a| a.get("width"))
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0) as i32;
                        let height = img
                            .get("aspectRatio")
                            .and_then(|a| a.get("height"))
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0) as i32;
                        let url = format!(
                            "https://cdn.bsky.app/img/feed_fullsize/plain/{}/{}",
                            did, cid
                        );
                        Some(ParsedAttachment {
                            url,
                            mime_type,
                            width,
                            height,
                            thumbnail_url: None,
                            is_gif: false,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        "app.bsky.embed.video" => {
            let Some(cid) = embed
                .get("video")
                .and_then(|v| v.get("ref"))
                .and_then(|r| r.get("$link"))
                .and_then(|v| v.as_str())
            else {
                return vec![];
            };
            let width = embed
                .get("aspectRatio")
                .and_then(|a| a.get("width"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as i32;
            let height = embed
                .get("aspectRatio")
                .and_then(|a| a.get("height"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as i32;
            let did_encoded = urlencoding::encode(did);
            let url = format!(
                "https://video.bsky.app/watch/{}/{}/playlist.m3u8",
                did_encoded, cid
            );
            let thumbnail_url = format!(
                "https://video.bsky.app/watch/{}/{}/thumbnail.jpg",
                did_encoded, cid
            );
            // GIFファイルを直接アップロードした場合、Bluesky動画パイプラインでMP4に
            // トランスコードされつつ`presentation:"gif"`が付与される（Tenor/Klipyの
            // GIFピッカーとは別経路）。
            let is_gif = embed.get("presentation").and_then(|v| v.as_str()) == Some("gif");
            vec![ParsedAttachment {
                url,
                mime_type: "application/vnd.apple.mpegurl".to_string(),
                width,
                height,
                thumbnail_url: Some(thumbnail_url),
                is_gif,
            }]
        }
        "app.bsky.embed.external" => embed
            .get("external")
            .and_then(|external| external.get("uri"))
            .and_then(|uri| uri.as_str())
            .and_then(|uri| bsky_gif_video_attachment(uri, embed, did))
            .into_iter()
            .collect(),
        "app.bsky.embed.recordWithMedia" => embed
            .get("media")
            .map(|media| parse_bsky_embed_attachments(media, did))
            .unwrap_or_default(),
        _ => vec![],
    }
}

/// `record.embed` から引用先の at:// URI を抽出する（#116）。
/// `app.bsky.embed.record` → `record.uri`
/// `app.bsky.embed.recordWithMedia`（引用+メディア）→ `record.record.uri`
///   （`record` フィールドの中にさらに `record` がネストする形）
/// 添付を持たない未知の embed 種別・`external` 等は `None` を返す。
pub fn parse_bsky_embed_quote_uri(embed: &JsonValue) -> Option<String> {
    let embed_type = embed.get("$type").and_then(|v| v.as_str()).unwrap_or("");
    match embed_type {
        "app.bsky.embed.record" => embed
            .get("record")?
            .get("uri")?
            .as_str()
            .map(|s| s.to_string()),
        "app.bsky.embed.recordWithMedia" => embed
            .get("record")?
            .get("record")?
            .get("uri")?
            .as_str()
            .map(|s| s.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn klipy_external_embed_is_parsed_as_mp4_attachment() {
        let embed = serde_json::json!({
            "$type": "app.bsky.embed.external",
            "external": {
                "uri": "https://static.klipy.com/ii/hash/af/33/file.gif?hh=498&ww=374&mp4=mp4slug&webm=webmslug",
                "thumb": {
                    "ref": { "$link": "thumbcid" }
                }
            }
        });

        let attachments = parse_bsky_embed_attachments(&embed, "did:plc:gm5vptmm3thf3vtzla5brxdd");

        assert_eq!(attachments.len(), 1);
        assert_eq!(
            attachments[0].url,
            "https://k.gifs.bsky.app/ii/hash/af/33/mp4slug.mp4"
        );
        assert_eq!(attachments[0].mime_type, "video/mp4");
        assert_eq!(attachments[0].width, 374);
        assert_eq!(attachments[0].height, 498);
        assert_eq!(
            attachments[0].thumbnail_url.as_deref(),
            Some(
                "https://cdn.bsky.app/img/feed_thumbnail/plain/did%3Aplc%3Agm5vptmm3thf3vtzla5brxdd/thumbcid"
            )
        );
    }

    #[test]
    fn tenor_external_embed_is_parsed_as_mp4_attachment() {
        let embed = serde_json::json!({
            "$type": "app.bsky.embed.external",
            "external": {
                "uri": "https://media.tenor.com/abcAAAAC/kitten.gif?hh=200&ww=300"
            }
        });

        let attachments = parse_bsky_embed_attachments(&embed, "did:plc:test");

        assert_eq!(attachments.len(), 1);
        assert_eq!(
            attachments[0].url,
            "https://t.gifs.bsky.app/abcAAAP1/kitten.mp4"
        );
        assert_eq!(attachments[0].mime_type, "video/mp4");
        assert_eq!(attachments[0].width, 300);
        assert_eq!(attachments[0].height, 200);
    }

    #[test]
    fn ordinary_external_embed_is_not_treated_as_media() {
        let embed = serde_json::json!({
            "$type": "app.bsky.embed.external",
            "external": {
                "uri": "https://example.com/article?hh=200&ww=300"
            }
        });

        assert!(parse_bsky_embed_attachments(&embed, "did:plc:test").is_empty());
    }

    #[test]
    fn bsky_record_embed_extracts_quote_uri() {
        let embed = serde_json::json!({
            "$type": "app.bsky.embed.record",
            "record": { "uri": "at://did:plc:alice/app.bsky.feed.post/quoted" }
        });
        assert_eq!(
            parse_bsky_embed_quote_uri(&embed).as_deref(),
            Some("at://did:plc:alice/app.bsky.feed.post/quoted")
        );
    }

    #[test]
    fn bsky_record_with_media_extracts_nested_quote_uri() {
        let embed = serde_json::json!({
            "$type": "app.bsky.embed.recordWithMedia",
            "record": {
                "$type": "app.bsky.embed.record",
                "record": { "uri": "at://did:plc:alice/app.bsky.feed.post/quoted" }
            },
            "media": { "$type": "app.bsky.embed.images", "images": [] }
        });
        assert_eq!(
            parse_bsky_embed_quote_uri(&embed).as_deref(),
            Some("at://did:plc:alice/app.bsky.feed.post/quoted")
        );
    }
}

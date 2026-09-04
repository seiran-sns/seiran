//! `seiranPost`拡張オブジェクト（他seiranサーバー間の投稿完全再現、未実装分の実装、#237）。
//!
//! AP Note・ATP post本体の両方に同一構造で埋め込む拡張オブジェクト。標準フィールド
//! （AP標準のNote・ATP標準のpost）は非対応の他実装（Mastodon/Misskey/Bluesky公式等）
//! 向けの互換表現として維持しつつ、これを検出した受信側seiranは標準フィールドを無視して
//! `posts`行を再構築する（無ければ標準フィールドのベストエフォート変換にフォールバック）。
//! 詳細: `docs/protocols.md` 5節。
//!
//! 意図的に含めないもの（詳細は#237本文参照）: reply/quote/repost先の参照
//! （標準のAP/ATP参照機構にそのまま委ねる）、DMのスレッド起点情報、返信・引用制限、
//! `linkCards[].embedSrc`/`embedType`（受信側は自分のホワイトリストで再解決する）、
//! `attachments[].altText`（未実装機能のため）、バージョニング、投稿本体レベルの`isSensitive`。

use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

/// `seiranPost.attachments[]`の1件分。
///
/// フィールドはDAG-CBOR canonical順（キーのバイト長→辞書順、`atp/repo.rs`の規約）で
/// 宣言する: url(3) < kind(4) < isGif(5) < width(5) < height(6) < blurhash(8) <
/// mimeType(8) < isSensitive(11)。JSON側（AP埋め込み）は宣言順に依存しないため、
/// このRust構造体1つを両プロトコルで共用できる。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeiranPostAttachment {
    pub url: String,
    /// "image" | "video" | "audio"
    pub kind: String,
    #[serde(rename = "isGif")]
    pub is_gif: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blurhash: Option<String>,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(rename = "isSensitive")]
    pub is_sensitive: bool,
}

/// `seiranPost.linkCards[]`の1件分。
/// canonical順: url(3) < title(5) < description(11) < thumbnailUrl(12)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeiranPostLinkCard {
    pub url: String,
    pub title: String,
    pub description: String,
    #[serde(rename = "thumbnailUrl", skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
}

/// `seiranPost`拡張オブジェクト本体。
///
/// フィールドはDAG-CBOR canonical順で宣言する: body(4) < poll(4) < emojiMap(8) <
/// language(8) < linkCards(9) < visibility(10) < attachments(11) < contentWarning(14) <
/// counterpartPostId(17) < counterpartAuthorId(19)。ATP post record本体への埋め込み
/// （`atp::repo::encode_bsky_feed_post`）はこの宣言順のままDAG-CBORへ直列化されるため、
/// 順序を崩すとcanonical CBORでなくなりCIDが不安定になる（`docs/protocols.md`参照）。
/// AP Note埋め込み（JSON）は宣言順に依存しないため影響しない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeiranPost {
    /// 変形前の生プレーンテキスト（Single Source of Truth）。
    pub body: String,
    /// `posts.poll`のJSONBをそのまま。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll: Option<serde_json::Value>,
    /// `:shortcode:` → 画像URL。`posts.emoji_map`をそのまま。
    #[serde(rename = "emojiMap")]
    pub emoji_map: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(rename = "linkCards")]
    pub link_cards: Vec<SeiranPostLinkCard>,
    /// "public" | "unlisted" | "followers_only" | "direct"
    pub visibility: String,
    pub attachments: Vec<SeiranPostAttachment>,
    #[serde(rename = "contentWarning", skip_serializing_if = "Option::is_none")]
    pub content_warning: Option<String>,
    /// この投稿自身が持つ、相手プロトコルでの真正なID（AP object id または AT URI）。
    /// ATP側コミットが未確定の間はこのフィールド自体を省略する（配送側の制約、
    /// `docs/protocols.md` 5節「配送側の制約（非対称、後から`Update`で補完）」参照）。
    #[serde(rename = "counterpartPostId", skip_serializing_if = "Option::is_none")]
    pub counterpart_post_id: Option<String>,
    /// この投稿の投稿者が持つ、相手プロトコルでの真正なID（AP actor URI または AT DID）。
    /// ローカルseiranユーザーは登録時点で両プロトコルのIDを常に持つため、投稿作成と
    /// 同時に必ず確定している（`counterpartPostId`と異なり配送を待つ必要がない）。
    #[serde(rename = "counterpartAuthorId")]
    pub counterpart_author_id: String,
}

/// MIMEタイプ前置詞から`seiranPost.attachments[].kind`の値を導出する。
/// seiranのローカル添付は image/video/audio のいずれかのみ（`docs/database.md`参照）。
pub fn media_kind_from_mime(mime_type: &str) -> &'static str {
    if mime_type.starts_with("video/") {
        "video"
    } else if mime_type.starts_with("audio/") {
        "audio"
    } else {
        "image"
    }
}

/// ローカル投稿の添付ファイル・URLリンクカードを`seiranPost`用の形へまとめて取得する。
/// AP送信（`ap::deliver::activity::build_seiran_post_for_basis`）・ATP送信の両方から使う
/// 共通クエリ（`post_attachments`+`media_files`+`storage_providers`、`post_link_cards`）。
pub async fn fetch_attachments_and_link_cards(
    db: &PgPool,
    post_id: i64,
) -> Result<(Vec<SeiranPostAttachment>, Vec<SeiranPostLinkCard>), sqlx::Error> {
    let attachment_rows = sqlx::query(
        "SELECT mf.mime_type, mf.width, mf.height, mf.blurhash, mf.storage_key, sp.public_url,
                pa.is_sensitive, pa.is_gif
         FROM post_attachments pa
         JOIN media_files mf ON mf.id = pa.media_file_id
         JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         WHERE pa.post_id = $1
         ORDER BY pa.position",
    )
    .bind(post_id)
    .fetch_all(db)
    .await?;

    let attachments = attachment_rows
        .iter()
        .filter_map(|r| {
            let mime_type: String = r.try_get("mime_type").ok()?;
            let storage_key: String = r.try_get("storage_key").ok()?;
            let public_url: String = r.try_get("public_url").ok()?;
            let url = format!("{}/{}", public_url.trim_end_matches('/'), storage_key);
            Some(SeiranPostAttachment {
                url,
                kind: media_kind_from_mime(&mime_type).to_string(),
                is_sensitive: r.try_get("is_sensitive").unwrap_or(false),
                is_gif: r.try_get("is_gif").unwrap_or(false),
                mime_type,
                width: r.try_get("width").ok().flatten(),
                height: r.try_get("height").ok().flatten(),
                blurhash: r.try_get("blurhash").ok().flatten(),
            })
        })
        .collect();

    let link_card_rows = sqlx::query(
        "SELECT url, title, description, thumbnail_url
         FROM post_link_cards
         WHERE post_id = $1
         ORDER BY position",
    )
    .bind(post_id)
    .fetch_all(db)
    .await?;

    let link_cards = link_card_rows
        .iter()
        .filter_map(|r| {
            Some(SeiranPostLinkCard {
                url: r.try_get("url").ok()?,
                title: r.try_get("title").ok()?,
                description: r.try_get("description").ok()?,
                thumbnail_url: r.try_get("thumbnail_url").ok().flatten(),
            })
        })
        .collect();

    Ok((attachments, link_cards))
}

impl SeiranPost {
    /// AP Note の `seiranPost` フィールド、または ATP post record の `seiranPost`
    /// フィールドから抽出する。フィールド自体が無い・型が合わない場合は`None`
    /// （標準フィールドへのベストエフォート変換フォールバックを呼び出し側が行う）。
    pub fn extract(obj: &serde_json::Value) -> Option<Self> {
        serde_json::from_value(obj.get("seiranPost")?.clone()).ok()
    }

    /// `serde_json::Value`へ変換する（AP Note / ATP post record への埋め込み用）。
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SeiranPost {
        SeiranPost {
            body: "こんにちは".to_string(),
            language: Some("ja".to_string()),
            visibility: "public".to_string(),
            content_warning: None,
            emoji_map: serde_json::json!({}),
            poll: None,
            counterpart_post_id: Some("did:plc:abc/app.bsky.feed.post/xyz".to_string()),
            counterpart_author_id: "did:plc:abc".to_string(),
            attachments: vec![],
            link_cards: vec![],
        }
    }

    #[test]
    fn roundtrips_through_json() {
        let post = sample();
        let value = post.to_value();
        assert_eq!(value["counterpartPostId"], "did:plc:abc/app.bsky.feed.post/xyz");
        assert_eq!(value["counterpartAuthorId"], "did:plc:abc");
        let parsed = SeiranPost::extract(&serde_json::json!({ "seiranPost": value })).unwrap();
        assert_eq!(parsed, post);
    }

    #[test]
    fn omits_counterpart_post_id_when_unconfirmed() {
        let mut post = sample();
        post.counterpart_post_id = None;
        let value = post.to_value();
        assert!(!value.as_object().unwrap().contains_key("counterpartPostId"));
    }

    #[test]
    fn extract_returns_none_when_absent() {
        assert!(SeiranPost::extract(&serde_json::json!({"type": "Note"})).is_none());
    }

    #[test]
    fn extract_returns_none_when_missing_required_field() {
        // counterpartAuthorId が無い不正な形は None（フォールバック対象）にする。
        let broken = serde_json::json!({
            "seiranPost": {
                "body": "x",
                "visibility": "public",
                "emojiMap": {},
                "attachments": [],
                "linkCards": []
            }
        });
        assert!(SeiranPost::extract(&broken).is_none());
    }
}

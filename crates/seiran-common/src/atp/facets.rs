//! `app.bsky.richtext.facet`（リンク・メンション・タグ）の解析。
//!
//! Jetstream経由の通常投稿取り込み（`seiran-atp-repo`）と、`com.atproto.repo.createRecord`
//! 等で受け付けたローカル投稿レコード（`seiran-api`）の両方で必要になるため、ここに
//! 共通実装を置く。

use serde::Deserialize;
use serde_json::Value as JsonValue;

/// `app.bsky.richtext.facet` の index（UTF-8 バイトオフセット）。
#[derive(Deserialize)]
pub struct ParsedFacetIndex {
    #[serde(rename = "byteStart")]
    pub byte_start: usize,
    #[serde(rename = "byteEnd")]
    pub byte_end: usize,
}

/// facet の feature 種別（`$type` で判別）。未知の種別はパース全体を失敗させないよう
/// `Unknown` に落とす。
#[derive(Deserialize)]
#[serde(tag = "$type")]
pub enum ParsedFacetFeature {
    #[serde(rename = "app.bsky.richtext.facet#link")]
    Link { uri: String },
    #[serde(rename = "app.bsky.richtext.facet#mention")]
    Mention { did: String },
    #[serde(rename = "app.bsky.richtext.facet#tag")]
    Tag,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
pub struct ParsedFacet {
    pub index: ParsedFacetIndex,
    pub features: Vec<ParsedFacetFeature>,
}

/// mention facet 1件分の位置情報（本文は書き換えず、別途 `posts.mention_facets` へ
/// 保存する。ハンドルは可変なので表示時（`NoteResponse` 生成時）に都度解決する）。
pub struct MentionFacetSpan {
    pub byte_start: usize,
    pub byte_end: usize,
    pub did: String,
}

/// `#link` facet が示すテキスト範囲を、内部リンクマーカー `[表示テキスト](URL)`
/// （Markdownリンク記法）に書き換える。URL は不変なのでここで確定してよい。
///
/// `#mention` facet は本文を書き換えない（メンション先のハンドルは DID 解決状況や
/// ハンドル変更で変わりうるため、表示時に都度解決する方針。フロントの MFM 描画コンポーネント
/// が `@user@host` パターンを自動でプロフィールリンクに変換するので、Markdownリンクで
/// 包む必要も無い）。代わりに `(byteStart, byteEnd, did)` を戻り値として返す。
/// `#tag` facet も無変換（`#tag` は既に地の文にありフロント側の自動検出に委ねる）。
///
/// `byteStart`/`byteEnd` は他 PDS から届く未検証の値のため、範囲外・非文字境界・他 facet
/// との重なりはそのfacetだけスキップする（投稿保存自体を失敗させない）。DB アクセスを含まない
/// 純粋関数にしてあるため単体テストしやすい。
pub fn apply_link_facets(text: &str, facets: Vec<ParsedFacet>) -> (String, Vec<MentionFacetSpan>) {
    if facets.is_empty() {
        return (text.to_string(), Vec::new());
    }

    // mention facet はテキストを変更しないため、位置情報だけ先に抜き出しておく
    // （範囲外・非文字境界のものは保存対象から除外）。
    let mut mention_spans = Vec::new();
    for facet in &facets {
        let start = facet.index.byte_start;
        let end = facet.index.byte_end;
        if start >= end
            || end > text.len()
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            continue;
        }
        for feature in &facet.features {
            if let ParsedFacetFeature::Mention { did } = feature {
                mention_spans.push(MentionFacetSpan {
                    byte_start: start,
                    byte_end: end,
                    did: did.clone(),
                });
            }
        }
    }

    // 以降は #link facet のみを対象に、後ろから順に本文へ焼き込む。
    let mut link_facets: Vec<ParsedFacet> = facets
        .into_iter()
        .filter(|f| {
            f.features
                .iter()
                .any(|feat| matches!(feat, ParsedFacetFeature::Link { .. }))
        })
        .collect();
    link_facets.sort_by_key(|f| std::cmp::Reverse(f.index.byte_start));

    let mut result = text.to_string();
    let mut upper_bound = result.len();

    for facet in link_facets {
        let start = facet.index.byte_start;
        let end = facet.index.byte_end;
        if start >= end || end > result.len() || end > upper_bound {
            continue;
        }
        if !result.is_char_boundary(start) || !result.is_char_boundary(end) {
            continue;
        }

        let Some(ParsedFacetFeature::Link { uri }) = facet
            .features
            .into_iter()
            .find(|f| matches!(f, ParsedFacetFeature::Link { .. }))
        else {
            continue;
        };

        let original = result[start..end].to_string();
        let replacement = format!("[{}]({})", original, uri);
        result.replace_range(start..end, &replacement);
        upper_bound = start;
    }

    (result, mention_spans)
}

/// facet を本文へ適用する（`apply_link_facets` のJSON組み立て込み版）。
/// 戻り値は `(link 適用済み本文, mention_facets の JSON 配列)`。
///
/// メンション先DIDは呼び出し側で先行upsertしない方針（#216）。表示時（`NoteResponse` 生成時）
/// に既知（他経路で保存済み）なDIDのみ解決され、未知のDIDはメンション元テキストのまま表示される。
pub fn apply_bsky_facets(text: &str, facets: Vec<ParsedFacet>) -> (String, JsonValue) {
    if facets.is_empty() {
        return (text.to_string(), JsonValue::Array(vec![]));
    }

    let (body, mention_spans) = apply_link_facets(text, facets);

    let mention_facets_json = JsonValue::Array(
        mention_spans
            .iter()
            .map(|s| {
                serde_json::json!({
                    "byteStart": s.byte_start,
                    "byteEnd": s.byte_end,
                    "did": s.did,
                })
            })
            .collect(),
    );

    (body, mention_facets_json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link_facet(byte_start: usize, byte_end: usize, uri: &str) -> ParsedFacet {
        ParsedFacet {
            index: ParsedFacetIndex {
                byte_start,
                byte_end,
            },
            features: vec![ParsedFacetFeature::Link {
                uri: uri.to_string(),
            }],
        }
    }

    fn mention_facet(byte_start: usize, byte_end: usize, did: &str) -> ParsedFacet {
        ParsedFacet {
            index: ParsedFacetIndex {
                byte_start,
                byte_end,
            },
            features: vec![ParsedFacetFeature::Mention {
                did: did.to_string(),
            }],
        }
    }

    #[test]
    fn single_link_facet_becomes_markdown_link() {
        let text = "見て example.com だよ";
        let byte_start = text.find("example.com").unwrap();
        let byte_end = byte_start + "example.com".len();
        let facets = vec![link_facet(byte_start, byte_end, "https://example.com")];
        let (result, mentions) = apply_link_facets(text, facets);
        assert_eq!(result, "見て [example.com](https://example.com) だよ");
        assert!(mentions.is_empty());
    }

    #[test]
    fn multiple_facets_applied_back_to_front_preserve_offsets() {
        let text = "foo.com and bar.com";
        let foo_start = text.find("foo.com").unwrap();
        let foo_end = foo_start + "foo.com".len();
        let bar_start = text.find("bar.com").unwrap();
        let bar_end = bar_start + "bar.com".len();
        // わざと昇順で渡し、関数側のソートが正しく後ろから処理することを確認する。
        let facets = vec![
            link_facet(foo_start, foo_end, "https://foo.com"),
            link_facet(bar_start, bar_end, "https://bar.com"),
        ];
        let (result, _) = apply_link_facets(text, facets);
        assert_eq!(
            result,
            "[foo.com](https://foo.com) and [bar.com](https://bar.com)"
        );
    }

    #[test]
    fn mention_facet_does_not_rewrite_body_but_is_extracted() {
        // メンションは本文を書き換えない（ハンドルは可変なので表示時に都度解決する）。
        // 呼び出し側が byteStart/byteEnd/did を mention_facets として保存できるよう返す。
        let text = "hi @alice.bsky.social and @unknown.bsky.social";
        let alice_start = text.find("@alice.bsky.social").unwrap();
        let alice_end = alice_start + "@alice.bsky.social".len();
        let unknown_start = text.find("@unknown.bsky.social").unwrap();
        let unknown_end = unknown_start + "@unknown.bsky.social".len();
        let facets = vec![
            mention_facet(alice_start, alice_end, "did:plc:alice"),
            mention_facet(unknown_start, unknown_end, "did:plc:unknown"),
        ];
        let (result, mentions) = apply_link_facets(text, facets);
        assert_eq!(result, text, "mention facet は本文を変更しない");
        assert_eq!(mentions.len(), 2);
        assert_eq!(mentions[0].did, "did:plc:alice");
        assert_eq!(mentions[0].byte_start, alice_start);
        assert_eq!(mentions[0].byte_end, alice_end);
        assert_eq!(mentions[1].did, "did:plc:unknown");
    }

    #[test]
    fn tag_only_facet_is_left_unchanged() {
        let text = "#rust最高";
        let byte_end = "#rust".len();
        let facets = vec![ParsedFacet {
            index: ParsedFacetIndex {
                byte_start: 0,
                byte_end,
            },
            features: vec![ParsedFacetFeature::Tag],
        }];
        let (result, mentions) = apply_link_facets(text, facets);
        assert_eq!(result, text);
        assert!(mentions.is_empty());
    }

    #[test]
    fn out_of_range_facet_is_skipped_without_panicking() {
        let text = "short";
        let facets = vec![link_facet(0, 1000, "https://example.com")];
        let (result, _) = apply_link_facets(text, facets);
        assert_eq!(result, text);
    }

    #[test]
    fn non_char_boundary_facet_is_skipped_without_panicking() {
        // "あ" は UTF-8 で3バイト。境界外の1バイト目を指定してもパニックしないこと。
        let text = "あいう";
        let facets = vec![link_facet(1, 2, "https://example.com")];
        let (result, _) = apply_link_facets(text, facets);
        assert_eq!(result, text);
    }

    #[test]
    fn overlapping_facets_second_one_is_skipped() {
        let text = "abcdef";
        // [0,4) と [2,6) が重なる。降順ソートで先に [2,6) が処理され、
        // 後続の [0,4) は upper_bound (=2) を超えるためスキップされる。
        let facets = vec![
            link_facet(0, 4, "https://a.example.com"),
            link_facet(2, 6, "https://b.example.com"),
        ];
        let (result, _) = apply_link_facets(text, facets);
        assert_eq!(result, "ab[cdef](https://b.example.com)");
    }

    #[test]
    fn out_of_range_mention_facet_is_dropped_without_panicking() {
        let text = "short";
        let facets = vec![mention_facet(0, 1000, "did:plc:x")];
        let (result, mentions) = apply_link_facets(text, facets);
        assert_eq!(result, text);
        assert!(mentions.is_empty());
    }
}

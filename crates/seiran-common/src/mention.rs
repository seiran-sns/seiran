//! 投稿本文中のメンション変換モジュール
//!
//! Bsky 配信用（AT Protocol ハンドルへの変換）と
//! AP 配信用（AT Protocol ハンドルを Fediverse メンション形式へ変換）の
//! 2 種類の変換関数を提供する。
//!
//! いずれも変換エラーが発生した場合は元のテキストをそのまま返す（ベストエフォート）。

use std::time::Duration;

use sqlx::{PgPool, Row};

use crate::atp::fetch_bsky_profile;
use crate::atp::repo::{BskyFacet, BskyFacetFeature, BskyFacetIndex, BskyFacetLink, BskyFacetMention, BskyFacetTag};

// ─────────────────────────────────────────────────────────────────────────────
// Bsky 向けメンション変換
// ─────────────────────────────────────────────────────────────────────────────

/// 投稿本文中の `@xxx` 形式メンションを AT Protocol ハンドルへ変換し、
/// 変換後テキストと AT Protocol Facet リストを返す。
///
/// * `@username`（ドメインなし）→ ローカルアクター確認後 `@username.{local_domain}` に展開
/// * `@handle.tld`（AT Protocol ハンドル形式）→ テキストはそのまま。`.{local_domain}` 接尾なら
///   ローカルユーザーとして、それ以外は公開 AppView `getProfile` で DID を解決する
/// * `@user@domain.tld` → brid.gy 2 段階ルックアップ → `@user.domain.tld.ap.brid.gy`
///   失敗した場合はテキストはそのままだが、既知の fedi アクターの本拠地 URL（無ければ自ドメインの
///   リモートプロフィールページ）への URL facet を付ける
///
/// DID/URL が取得できた場合は対応する `BskyFacet` を生成する（ベストエフォート）。
/// 変換中に DB / HTTP エラーが発生した場合は元テキストにフォールバックする。
pub async fn convert_mentions_for_bsky(
    text: &str,
    local_domain: &str,
    pool: &PgPool,
    http_client: &reqwest::Client,
) -> (String, Vec<BskyFacet>) {
    let text_chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len() * 2);
    let mut facets: Vec<BskyFacet> = Vec::new();
    let mut i = 0;

    while i < text_chars.len() {
        let ch = text_chars[i];

        if ch == 'h' {
            if let Some(end) = scan_url(&text_chars, i) {
                let url: String = text_chars[i..end].iter().collect();
                let byte_start = result.len();
                result.push_str(&url);
                let byte_end = result.len();
                facets.push(make_link_facet(byte_start, byte_end, url));
                i = end;
                continue;
            }
        }

        if ch == '#' {
            if let Some((tag_body, end)) = scan_hashtag(&text_chars, i) {
                let byte_start = result.len();
                result.push('#');
                result.push_str(&tag_body);
                let byte_end = result.len();
                facets.push(make_tag_facet(byte_start, byte_end, tag_body));
                i = end;
                continue;
            }
        }

        if ch != '@' {
            result.push(ch);
            i += 1;
            continue;
        }

        // `@` の直前が半角英数字 / アンダースコアならメールアドレスの一部としてスキップ。
        // ASCII限定でチェックするのは、`is_alphanumeric()`（Unicode版）だと日本語等の文字も
        // 真になり、「文章@handle」のようにCJK文字に直接続くメンションを誤ってスキップして
        // しまうため（実機確認: 全角括弧直後にスペース無しで `@ethilen.bsky.social` と続く投稿）。
        if i > 0 {
            let prev = text_chars[i - 1];
            if prev.is_ascii_alphanumeric() || prev == '_' {
                result.push('@');
                i += 1;
                continue;
            }
        }

        i += 1; // skip '@'

        // 識別子部分を読む（英数字・アンダースコア・ハイフン・ドット）。
        // ドットを含めるのは `@handle.tld`（AT Protocol ハンドル形式）を
        // 後続の分岐で見分けるため（`@user@domain` 形式ではユーザー名部分に
        // ドットは通常含まれないため、この拡張と衝突しない）。
        let username_start = i;
        while i < text_chars.len()
            && (text_chars[i].is_alphanumeric()
                || text_chars[i] == '_'
                || text_chars[i] == '-'
                || text_chars[i] == '.')
        {
            i += 1;
        }

        if i == username_start {
            // `@` の直後に有効な文字がない
            result.push('@');
            continue;
        }

        let username: String = text_chars[username_start..i].iter().collect();

        // `@handle.tld` 形式（AT Protocol ハンドルとして書かれたメンション）か判定
        let is_atproto_handle = {
            let parts: Vec<&str> = username.split('.').collect();
            parts.len() >= 2 && parts.last().map(|t| t.len() >= 2).unwrap_or(false)
        };

        // `@user@domain` 形式か確認
        if i < text_chars.len() && text_chars[i] == '@' {
            i += 1; // skip second '@'
            let domain_start = i;
            while i < text_chars.len()
                && (text_chars[i].is_alphanumeric() || text_chars[i] == '.' || text_chars[i] == '-')
            {
                i += 1;
            }

            if i == domain_start {
                // ドメイン部が空 → そのまま出力
                result.push('@');
                result.push_str(&username);
                result.push('@');
                continue;
            }

            let domain: String = text_chars[domain_start..i].iter().collect();

            // domain == local_domain の場合はローカルユーザーとして扱う
            if domain.eq_ignore_ascii_case(local_domain) {
                let did = get_local_actor_did(&username, pool).await;
                let byte_start = result.len();
                result.push('@');
                result.push_str(&username);
                result.push('.');
                result.push_str(local_domain);
                let byte_end = result.len();
                if let Some(did) = did {
                    facets.push(make_mention_facet(byte_start, byte_end, did));
                }
            } else {
                // brid.gy 経由で Bsky ハンドルと DID を解決
                match resolve_fedi_for_bsky(&username, &domain, pool, http_client).await {
                    Some((handle, did_opt)) => {
                        let byte_start = result.len();
                        result.push('@');
                        result.push_str(&handle);
                        let byte_end = result.len();
                        if let Some(did) = did_opt {
                            facets.push(make_mention_facet(byte_start, byte_end, did));
                        }
                    }
                    None => {
                        // ブリッジ解決失敗 → テキストは `@user@domain` のまま変更しないが、
                        // Bsky はメンションと認識できないため代わりに URL facet を付ける。
                        // 既知の fedi アクター（DB に行がある）なら本拠地 URL（ap_uri）、
                        // 無ければ自ドメインのリモートプロフィールページへリンクする。
                        let byte_start = result.len();
                        result.push('@');
                        result.push_str(&username);
                        result.push('@');
                        result.push_str(&domain);
                        let byte_end = result.len();

                        let link_url = match get_fedi_actor_home_url(&username, &domain, pool).await {
                            Some(home_url) => home_url,
                            None => format!("https://{}/@{}@{}", local_domain, username, domain),
                        };
                        facets.push(make_link_facet(byte_start, byte_end, link_url));
                    }
                }
            }
        } else if is_atproto_handle {
            let local_suffix = format!(".{}", local_domain);
            let did = if let Some(local_username) = username.strip_suffix(&local_suffix) {
                // 自ドメインの正規ハンドル形式（`{username}.{local_domain}`）→ ローカルユーザーとして解決
                get_local_actor_did(local_username, pool).await
            } else {
                // 外部 Bsky ハンドルとして AppView に解決を試みる
                resolve_bsky_handle_did(&username, http_client).await
            };
            let byte_start = result.len();
            result.push('@');
            result.push_str(&username);
            let byte_end = result.len();
            if let Some(did) = did {
                facets.push(make_mention_facet(byte_start, byte_end, did));
            }
        } else {
            // `@username` 形式 → ローカルアクター確認
            if is_local_actor(&username, pool).await {
                let did = get_local_actor_did(&username, pool).await;
                let byte_start = result.len();
                result.push('@');
                result.push_str(&username);
                result.push('.');
                result.push_str(local_domain);
                let byte_end = result.len();
                if let Some(did) = did {
                    facets.push(make_mention_facet(byte_start, byte_end, did));
                }
            } else {
                // ローカルに存在しない → そのまま
                result.push('@');
                result.push_str(&username);
            }
        }
    }

    (result, facets)
}

/// `text_chars[start..]` が `http://` または `https://` で始まる場合、URLの終端インデックス
/// （exclusive）を返す。空白と `< > ( ) [ ]` を区切り文字として扱う
/// （フロント `frontend/src/lib/richTextPatterns.ts` のURL正規表現と同じ区切り文字集合）。
fn scan_url(text_chars: &[char], start: usize) -> Option<usize> {
    let prefix_len = if text_chars[start..].starts_with(&['h', 't', 't', 'p', 's', ':', '/', '/']) {
        8
    } else if text_chars[start..].starts_with(&['h', 't', 't', 'p', ':', '/', '/']) {
        7
    } else {
        return None;
    };
    let mut end = start + prefix_len;
    while end < text_chars.len() {
        let c = text_chars[end];
        if c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')' | '[' | ']') {
            break;
        }
        end += 1;
    }
    // スキーム部分のみ（後続のホスト名等が無い）なら意味のあるURLとして扱わない
    if end == start + prefix_len {
        return None;
    }
    Some(end)
}

/// `text_chars[start]` が `#` である前提で、直前文字の境界チェックとタグ本体のスキャンを行う。
/// 有効なハッシュタグなら `(タグ本体（# を除く、大文字小文字保持）, 終端インデックス（exclusive）)`
/// を返す。境界・除外ルールは `crate::hashtag::extract_hashtags`（永続化用の抽出関数）と同じ
/// （直前が半角英数字/アンダースコア/`/` ならスキップ、本体にアルファベットを1文字も含まない
/// 場合は無効＝URLフラグメントや `#2026` 等の純数字列を誤検出しない）。大文字小文字を保持する点が
/// 抽出関数と異なる（グルーピング用の正規化は永続化層の責務であり、配信用テキストには影響しない）。
fn scan_hashtag(text_chars: &[char], start: usize) -> Option<(String, usize)> {
    if start > 0 {
        let prev = text_chars[start - 1];
        if prev.is_ascii_alphanumeric() || prev == '_' || prev == '/' {
            return None;
        }
    }
    let tag_start = start + 1;
    let mut end = tag_start;
    while end < text_chars.len() && (text_chars[end].is_alphanumeric() || text_chars[end] == '_') {
        end += 1;
    }
    if end == tag_start {
        return None;
    }
    let tag_body: String = text_chars[tag_start..end].iter().collect();
    if !tag_body.chars().any(|c| c.is_alphabetic()) {
        return None;
    }
    Some((tag_body, end))
}

/// `BskyFacet`（tag）を生成するヘルパー。
fn make_tag_facet(byte_start: usize, byte_end: usize, tag: String) -> BskyFacet {
    BskyFacet {
        index: BskyFacetIndex { byte_end, byte_start },
        features: vec![BskyFacetFeature::Tag(BskyFacetTag {
            tag,
            kind: "app.bsky.richtext.facet#tag".to_string(),
        })],
    }
}

/// `BskyFacet`（mention）を生成するヘルパー。
fn make_mention_facet(byte_start: usize, byte_end: usize, did: String) -> BskyFacet {
    BskyFacet {
        index: BskyFacetIndex { byte_end, byte_start },
        features: vec![BskyFacetFeature::Mention(BskyFacetMention {
            did,
            kind: "app.bsky.richtext.facet#mention".to_string(),
        })],
    }
}

/// `BskyFacet`（link）を生成するヘルパー。
fn make_link_facet(byte_start: usize, byte_end: usize, uri: String) -> BskyFacet {
    BskyFacet {
        index: BskyFacetIndex { byte_end, byte_start },
        features: vec![BskyFacetFeature::Link(BskyFacetLink {
            uri,
            kind: "app.bsky.richtext.facet#link".to_string(),
        })],
    }
}

/// `actors` テーブルから fedi アクター（`username`@`domain`）の本拠地 URL（`ap_uri`）を取得する。
///
/// DB に行が無い、または `ap_uri` が未設定の場合は `None` を返す。
async fn get_fedi_actor_home_url(username: &str, domain: &str, pool: &PgPool) -> Option<String> {
    let row = sqlx::query("SELECT ap_uri FROM actors WHERE username = $1 AND domain = $2 LIMIT 1")
        .bind(username)
        .bind(domain)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

    row.and_then(|r| r.try_get::<Option<String>, _>("ap_uri").ok().flatten())
}

/// `actors` テーブルからローカルアクターの `at_did` を取得する。
///
/// アクターが存在しない、または `at_did` が未設定の場合は `None` を返す。
async fn get_local_actor_did(username: &str, pool: &PgPool) -> Option<String> {
    let row = sqlx::query(
        "SELECT at_did FROM actors WHERE actor_type = 'local' AND username = $1 LIMIT 1",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    row.and_then(|r| r.try_get::<Option<String>, _>("at_did").ok().flatten())
}

/// `actors` テーブルにローカルアクター（`actor_type = 'local'`）として存在するか確認する。
async fn is_local_actor(username: &str, pool: &PgPool) -> bool {
    sqlx::query(
        "SELECT 1 FROM actors WHERE actor_type = 'local' AND username = $1 LIMIT 1",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
    .map(|opt| opt.is_some())
    .unwrap_or(false)
}

/// Fediverse メンション（`@user@domain`）を brid.gy 経由で AT Protocol ハンドルと DID に解決する。
///
/// 戻り値: `Some((handle, Option<did>))` または `None`（解決失敗）。
///
/// ハンドルの存在確認・DID取得は常に公開 AppView（`resolve_bsky_handle_did`）で行う
/// （`bsky.brid.gy` は `com.atproto.identity.resolveHandle` を実装していない
/// ＝ `MethodNotImplemented` を返すため使えない。実機確認）。DB は「ローカルに
/// 既知の brid.gy アクターとして記録済みか」の事前チェックにのみ使う。
async fn resolve_fedi_for_bsky(
    username: &str,
    domain: &str,
    pool: &PgPool,
    http_client: &reqwest::Client,
) -> Option<(String, Option<String>)> {
    // brid.gy ハンドル命名規則: {username}.{domain}.ap.brid.gy
    let bridgy_username = format!("{}.{}", username, domain);
    let bridgy_handle = format!("{}.ap.brid.gy", bridgy_username);

    let did = resolve_bsky_handle_did(&bridgy_handle, http_client).await;
    if did.is_some() {
        return Some((bridgy_handle, did));
    }

    // 公開 AppView で解決できなくても、DB に既知アクターとして記録済みなら
    // ハンドル自体は生きているとみなしテキストだけ変換する（facet無し）。
    let found_in_db = sqlx::query(
        "SELECT 1 FROM actors WHERE username = $1 AND domain = 'bsky.brid.gy' LIMIT 1",
    )
    .bind(&bridgy_username)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .is_some();

    if found_in_db {
        return Some((bridgy_handle, None));
    }

    None
}

/// AT Protocol ハンドル文字列から DID を解決する（公開 AppView `getProfile`、2 秒タイムアウト）。
/// brid.gy ブリッジハンドル（`user.domain.ap.brid.gy`）・「生の」Bsky ハンドル
/// （`alice.bsky.social` 等）のいずれも、公開 AppView は同じ `getProfile` で解決できる。
async fn resolve_bsky_handle_did(handle: &str, http_client: &reqwest::Client) -> Option<String> {
    match tokio::time::timeout(Duration::from_secs(2), fetch_bsky_profile(http_client, handle)).await {
        Ok(Ok(profile)) => Some(profile.did),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AP（Fediverse）向け ATP ハンドル変換
// ─────────────────────────────────────────────────────────────────────────────

/// スパンの種別。`tag[]` への追加要否・アンカーの `class`/`rel` 属性を左右する
/// （`plain_to_html_with_mentions`・`crate::ap::deliver` 参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApInlineSpanKind {
    /// AP `Mention` として `tag[]` に追加する（メンション通知対象）。
    Mention,
    /// AP `Hashtag` として `tag[]` に追加する。
    Hashtag,
    /// `tag[]` には追加しない単なるリンク（生URL、解決できなかった ATP ハンドルの
    /// bsky.app プロフィールへのリンク等）。
    Link,
}

/// AP 配信用の本文中インラインメンション/ハッシュタグ/リンク（1 スパン）。
/// `byte_start`/`byte_end` は `convert_mentions_for_ap` の戻り値テキストにおける
/// UTF-8 バイトオフセット（`plain_to_html_with_mentions` で `<a>` 化に使う）。
pub struct ApInlineMention {
    pub byte_start: usize,
    pub byte_end: usize,
    /// リンク先 URL（Mention なら AP actor の href、Hashtag なら自インスタンスのタグページ）。
    pub href: String,
    /// 表示テキスト（このスパンの本文と同じ）。
    pub name: String,
    pub kind: ApInlineSpanKind,
}

/// 投稿本文中のメンションを AP 配信向けに解決する。
///
/// * `@username`（ローカルユーザー、ドメイン省略） → `@username@{local_domain}` に qualify し、
///   ローカルアクターの AP actor URI を href とする Mention
/// * `@username.{local_domain}`（ローカルユーザーの AT Protocol ハンドル表記） → ローカル
///   ユーザーだとわかっているので `@username@{local_domain}`（Fediverse 表記）に変換する
///   （brid.gy 解決は試みない）
/// * `@user@domain`（Fediverse 形式） → テキストは変更しないが、DB（既知アクター）
///   または webfinger で href を解決できた場合のみ Mention を追加する
/// * `@handle.tld`（他インスタンスの AT Protocol ハンドル形式） → brid.gy 経由の webfinger で
///   解決できれば `@handle.tld@bsky.brid.gy` の Mention、できなければ bsky.app プロフィールへの
///   単なるリンク（`ApInlineSpanKind::Link`）
/// * `#タグ` → 自インスタンスのハッシュタグページ（`https://{local_domain}/tags/{タグ}`）への
///   Hashtag（`docs/protocols.md` 6節参照）
///
/// 変換中にエラーが発生した場合は元テキストにフォールバックする（対応する `ApInlineMention` は追加しない）。
pub async fn convert_mentions_for_ap(
    text: &str,
    local_domain: &str,
    pool: &PgPool,
    http_client: &reqwest::Client,
) -> (String, Vec<ApInlineMention>) {
    let text_chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len() * 2);
    let mut mentions: Vec<ApInlineMention> = Vec::new();
    let mut i = 0;

    while i < text_chars.len() {
        let ch = text_chars[i];

        if ch == 'h' {
            if let Some(end) = scan_url(&text_chars, i) {
                let url: String = text_chars[i..end].iter().collect();
                let byte_start = result.len();
                result.push_str(&url);
                let byte_end = result.len();
                mentions.push(ApInlineMention {
                    byte_start,
                    byte_end,
                    href: url.clone(),
                    name: url,
                    kind: ApInlineSpanKind::Link,
                });
                i = end;
                continue;
            }
        }

        if ch == '#' {
            if let Some((tag_body, end)) = scan_hashtag(&text_chars, i) {
                let name = format!("#{}", tag_body);
                let href = format!("https://{}/tags/{}", local_domain, tag_body.to_lowercase());
                let byte_start = result.len();
                result.push_str(&name);
                let byte_end = result.len();
                mentions.push(ApInlineMention { byte_start, byte_end, href, name, kind: ApInlineSpanKind::Hashtag });
                i = end;
                continue;
            }
        }

        if ch != '@' {
            result.push(ch);
            i += 1;
            continue;
        }

        // `@` の直前が半角英数字 / アンダースコアならメールアドレスの一部としてスキップ
        // （ASCII限定の理由は `convert_mentions_for_bsky` 側の同様のコメントを参照）
        if i > 0 {
            let prev = text_chars[i - 1];
            if prev.is_ascii_alphanumeric() || prev == '_' {
                result.push('@');
                i += 1;
                continue;
            }
        }

        i += 1; // skip '@'

        // `@` 直後の識別子を読む（英数字・アンダースコア・ハイフン・ドット）
        let ident_start = i;
        while i < text_chars.len()
            && (text_chars[i].is_alphanumeric()
                || text_chars[i] == '_'
                || text_chars[i] == '-'
                || text_chars[i] == '.')
        {
            i += 1;
        }

        if i == ident_start {
            result.push('@');
            continue;
        }

        let ident: String = text_chars[ident_start..i].iter().collect();

        // 直後に `@` が続く場合は Fediverse 形式（`@user@domain`）
        if i < text_chars.len() && text_chars[i] == '@' {
            i += 1; // skip second '@'
            let domain_start = i;
            while i < text_chars.len()
                && (text_chars[i].is_alphanumeric() || text_chars[i] == '.' || text_chars[i] == '-')
            {
                i += 1;
            }
            if i == domain_start {
                // ドメイン部が空 → そのまま出力
                result.push('@');
                result.push_str(&ident);
                result.push('@');
                continue;
            }
            let domain: String = text_chars[domain_start..i].iter().collect();
            let name = format!("@{}@{}", ident, domain);
            let byte_start = result.len();
            result.push_str(&name);
            let byte_end = result.len();
            if let Some(href) = resolve_fedi_mention_href(&ident, &domain, pool, http_client).await {
                mentions.push(ApInlineMention { byte_start, byte_end, href, name, kind: ApInlineSpanKind::Mention });
            }
            continue;
        }

        // ATP ハンドルとして扱う条件:
        //   - ドット（.）を含む（ドメイン形式）
        //   - 最後のラベル（TLD）が 2 文字以上
        let looks_like_atp_handle = {
            let parts: Vec<&str> = ident.split('.').collect();
            parts.len() >= 2 && parts.last().map(|t| t.len() >= 2).unwrap_or(false)
        };

        if looks_like_atp_handle {
            let local_suffix = format!(".{}", local_domain);
            if let Some(local_username) = ident.strip_suffix(&local_suffix) {
                // 自ドメインの正規ハンドル形式（`{username}.{local_domain}`）で書かれたローカル
                // ユーザーへのメンション → ローカルユーザーだとわかっているので Fediverse 表記
                // （`@username@local_domain`）に変換する（brid.gy 解決を試みる必要はない）。
                if is_local_actor(local_username, pool).await {
                    let name = format!("@{}@{}", local_username, local_domain);
                    let href = format!("https://{}/users/{}", local_domain, local_username);
                    let byte_start = result.len();
                    result.push_str(&name);
                    let byte_end = result.len();
                    mentions.push(ApInlineMention { byte_start, byte_end, href, name, kind: ApInlineSpanKind::Mention });
                } else {
                    result.push('@');
                    result.push_str(&ident);
                }
            } else {
                match resolve_fedi_mention_href(&ident, "bsky.brid.gy", pool, http_client).await {
                    Some(href) => {
                        let name = format!("@{}@bsky.brid.gy", ident);
                        let byte_start = result.len();
                        result.push_str(&name);
                        let byte_end = result.len();
                        mentions.push(ApInlineMention { byte_start, byte_end, href, name, kind: ApInlineSpanKind::Mention });
                    }
                    None => {
                        // brid.gy で解決できない → bsky.app プロフィールへの単なるリンクとして表示する
                        let name = ident.clone();
                        let href = format!("https://bsky.app/profile/{}", ident);
                        let byte_start = result.len();
                        result.push_str(&name);
                        let byte_end = result.len();
                        mentions.push(ApInlineMention { byte_start, byte_end, href, name, kind: ApInlineSpanKind::Link });
                    }
                }
            }
        } else if is_local_actor(&ident, pool).await {
            // ローカルユーザーへのメンション → 外部から見て意味を持つよう `@ident@local_domain` に qualify する
            let name = format!("@{}@{}", ident, local_domain);
            let href = format!("https://{}/users/{}", local_domain, ident);
            let byte_start = result.len();
            result.push_str(&name);
            let byte_end = result.len();
            mentions.push(ApInlineMention { byte_start, byte_end, href, name, kind: ApInlineSpanKind::Mention });
        } else {
            result.push('@');
            result.push_str(&ident);
        }
    }

    (result, mentions)
}

/// `ApInlineMention` 群から AP `tag[]` 配列を組み立てる（`Mention`/`Hashtag` のみ、
/// `Link` は含めない）。push配送（`crate::ap::deliver`）と pull取得
/// （`seiran-api::handlers::notes::get_note_ap`）の両方で同じ組み立てロジックを共有する
/// （`docs/protocols.md` 6節）。
pub fn ap_inline_mentions_to_tag_json(mentions: &[ApInlineMention]) -> Vec<serde_json::Value> {
    mentions
        .iter()
        .filter_map(|m| match m.kind {
            ApInlineSpanKind::Mention => {
                Some(serde_json::json!({"type": "Mention", "href": m.href, "name": m.name}))
            }
            ApInlineSpanKind::Hashtag => {
                Some(serde_json::json!({"type": "Hashtag", "href": m.href, "name": m.name}))
            }
            ApInlineSpanKind::Link => None,
        })
        .collect()
}

/// webfinger（`acct:{username}@{domain}`）で AP actor の href
/// （`rel=="self"` の `application/activity+json` リンク）を解決する（2 秒タイムアウト）。
async fn resolve_ap_actor_href_via_webfinger(
    username: &str,
    domain: &str,
    http_client: &reqwest::Client,
) -> Option<String> {
    let url = format!(
        "https://{}/.well-known/webfinger?resource=acct:{}@{}",
        domain, username, domain
    );
    let res = tokio::time::timeout(Duration::from_secs(2), http_client.get(&url).send()).await;
    let body = match res {
        Ok(Ok(response)) if response.status().is_success() => {
            response.json::<serde_json::Value>().await.ok()?
        }
        _ => return None,
    };
    body["links"].as_array()?.iter().find_map(|l| {
        let rel = l.get("rel").and_then(|v| v.as_str())?;
        if rel != "self" {
            return None;
        }
        let type_ok = l
            .get("type")
            .and_then(|v| v.as_str())
            .map(|t| t.contains("json"))
            .unwrap_or(true);
        if !type_ok {
            return None;
        }
        l.get("href").and_then(|v| v.as_str()).map(|s| s.to_string())
    })
}

/// fedi メンション（`username@domain`）の AP actor href を解決する。
/// DB に既知アクターの行があればその `ap_uri` を優先し、無ければ webfinger で確認する
/// （`domain` に `bsky.brid.gy` を渡せば ATP ハンドルの brid.gy ブリッジ確認にも使える）。
async fn resolve_fedi_mention_href(
    username: &str,
    domain: &str,
    pool: &PgPool,
    http_client: &reqwest::Client,
) -> Option<String> {
    if let Some(uri) = get_fedi_actor_home_url(username, domain, pool).await {
        return Some(uri);
    }
    resolve_ap_actor_href_via_webfinger(username, domain, http_client).await
}

// ─────────────────────────────────────────────────────────────────────────────
// テスト
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{scan_hashtag, scan_url};

    #[test]
    fn scan_url_detects_https() {
        let chars: Vec<char> = "https://example.com/path".chars().collect();
        let end = scan_url(&chars, 0).unwrap();
        let url: String = chars[0..end].iter().collect();
        assert_eq!(url, "https://example.com/path");
    }

    #[test]
    fn scan_url_detects_http() {
        let chars: Vec<char> = "http://example.com".chars().collect();
        let end = scan_url(&chars, 0).unwrap();
        let url: String = chars[0..end].iter().collect();
        assert_eq!(url, "http://example.com");
    }

    #[test]
    fn scan_url_stops_at_whitespace_and_delimiters() {
        for (input, expected) in [
            ("https://x.com 続き", "https://x.com"),
            ("https://x.com)続き", "https://x.com"),
            ("https://x.com]続き", "https://x.com"),
            ("https://x.com\n続き", "https://x.com"),
            // 区切り文字集合に無い文字（日本語等）はURLの一部として飲み込まれる
            // （フロント側の正規表現と同じ挙動。閉じ括弧の手前で止まる）
            ("(https://x.comを見て)", "https://x.comを見て"),
        ] {
            let chars: Vec<char> = input.chars().collect();
            let start = chars.iter().position(|&c| c == 'h').unwrap();
            let end = scan_url(&chars, start).unwrap();
            let url: String = chars[start..end].iter().collect();
            assert_eq!(url, expected, "input={:?}", input);
        }
    }

    #[test]
    fn scan_url_returns_none_for_non_url_h_word() {
        let chars: Vec<char> = "hello world".chars().collect();
        assert!(scan_url(&chars, 0).is_none());
    }

    #[test]
    fn scan_url_returns_none_for_bare_scheme() {
        // スキームだけで後続が無い/区切り文字が直後に来る場合は URL として扱わない
        let chars: Vec<char> = "https:// ".chars().collect();
        assert!(scan_url(&chars, 0).is_none());
    }

    /// メールアドレス埋め込み文字列において `@` をメンション開始と誤認しないことを確認する。
    /// （DB / HTTP を使わない純粋な構文テスト。実際のガードは ASCII 限定 `is_ascii_alphanumeric()`
    /// を使う、後続テストの `mention_guard_does_not_skip_at_after_cjk_char` 参照）
    #[test]
    fn email_at_sign_should_not_start_mention() {
        // `admin@example.com` の `@` は直前が半角英数字なのでスキップされる
        // このテストはロジック確認用のため、非同期部分は含まない
        let text = "contact admin@example.com please";
        let chars: Vec<char> = text.chars().collect();
        let mut skipped = false;
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '@' && i > 0 {
                let prev = chars[i - 1];
                if prev.is_ascii_alphanumeric() || prev == '_' {
                    skipped = true;
                }
            }
            i += 1;
        }
        assert!(skipped, "`@` in email should be detected as preceded by word char");
    }

    #[test]
    fn scan_hashtag_detects_ascii_tag() {
        let chars: Vec<char> = "#foo bar".chars().collect();
        let (tag, end) = scan_hashtag(&chars, 0).unwrap();
        assert_eq!(tag, "foo");
        assert_eq!(end, 4);
    }

    #[test]
    fn scan_hashtag_detects_japanese_tag_directly_after_text() {
        // 日本語はスペース無しで直接 `#タグ` が続くのが実利用上の通常パターン。
        let chars: Vec<char> = "今日も#猫 かわいい".chars().collect();
        let hash_idx = chars.iter().position(|&c| c == '#').unwrap();
        let (tag, _end) = scan_hashtag(&chars, hash_idx).unwrap();
        assert_eq!(tag, "猫");
    }

    #[test]
    fn scan_hashtag_rejects_url_fragment() {
        let chars: Vec<char> = "https://x.com/a#frag".chars().collect();
        let hash_idx = chars.iter().position(|&c| c == '#').unwrap();
        assert!(scan_hashtag(&chars, hash_idx).is_none());
    }

    #[test]
    fn scan_hashtag_rejects_pure_numeric() {
        let chars: Vec<char> = "#2026".chars().collect();
        assert!(scan_hashtag(&chars, 0).is_none());
    }

    /// CJK文字（日本語等）に直接続く `@mention` は、メールアドレスの一部として誤スキップされない
    /// ことを確認する（実機で「リモート@ethilen.bsky.social」のようにスペース無しで続く投稿が
    /// 完全に無処理になっていたバグの再発防止）。`is_alphanumeric()`（Unicode版）だと
    /// 日本語の文字も真になってしまうため、ガードは ASCII 限定でなければならない。
    #[test]
    fn mention_guard_does_not_skip_at_after_cjk_char() {
        let prev_char = 'ト'; // 「リモート」の末尾（カタカナ）
        assert!(prev_char.is_alphanumeric(), "カタカナは Unicode 版 is_alphanumeric() では真になる");
        assert!(
            !prev_char.is_ascii_alphanumeric(),
            "ASCII版では偽になり、@ をメンション開始として正しく認識できる"
        );
    }
}

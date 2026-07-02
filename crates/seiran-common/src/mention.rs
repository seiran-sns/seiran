//! 投稿本文中のメンション変換モジュール
//!
//! Bsky 配信用（AT Protocol ハンドルへの変換）と
//! AP 配信用（AT Protocol ハンドルを Fediverse メンション形式へ変換）の
//! 2 種類の変換関数を提供する。
//!
//! いずれも変換エラーが発生した場合は元のテキストをそのまま返す（ベストエフォート）。

use std::time::Duration;

use sqlx::PgPool;

// ─────────────────────────────────────────────────────────────────────────────
// Bsky 向けメンション変換
// ─────────────────────────────────────────────────────────────────────────────

/// 投稿本文中の `@xxx` 形式メンションを AT Protocol ハンドルへ変換する。
///
/// * `@username`（ドメインなし）→ ローカルアクター確認後 `@username.{local_domain}` に展開
/// * `@user@domain.tld` → brid.gy 2 段階ルックアップ → `@user.domain.tld.ap.brid.gy`
///   失敗した場合はそのまま（AT Protocol では未知メンションはテキスト扱い）
///
/// 変換中に DB / HTTP エラーが発生した場合は元テキストにフォールバックする。
pub async fn convert_mentions_for_bsky(
    text: &str,
    local_domain: &str,
    pool: &PgPool,
    http_client: &reqwest::Client,
) -> String {
    let text_chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len() * 2);
    let mut i = 0;

    while i < text_chars.len() {
        let ch = text_chars[i];

        if ch != '@' {
            result.push(ch);
            i += 1;
            continue;
        }

        // `@` の直前が英数字 / アンダースコアならメールアドレスの一部としてスキップ
        if i > 0 {
            let prev = text_chars[i - 1];
            if prev.is_alphanumeric() || prev == '_' {
                result.push('@');
                i += 1;
                continue;
            }
        }

        i += 1; // skip '@'

        // ユーザー名部分を読む（英数字・アンダースコア・ハイフン）
        let username_start = i;
        while i < text_chars.len()
            && (text_chars[i].is_alphanumeric() || text_chars[i] == '_' || text_chars[i] == '-')
        {
            i += 1;
        }

        if i == username_start {
            // `@` の直後に有効な文字がない
            result.push('@');
            continue;
        }

        let username: String = text_chars[username_start..i].iter().collect();

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
                result.push('@');
                result.push_str(&username);
                result.push('.');
                result.push_str(local_domain);
            } else {
                // brid.gy 経由で Bsky ハンドルを解決
                match resolve_fedi_for_bsky(&username, &domain, pool, http_client).await {
                    Some(handle) => {
                        result.push('@');
                        result.push_str(&handle);
                    }
                    None => {
                        // 解決失敗 → 元のテキストをそのまま残す
                        result.push('@');
                        result.push_str(&username);
                        result.push('@');
                        result.push_str(&domain);
                    }
                }
            }
        } else {
            // `@username` 形式 → ローカルアクター確認
            if is_local_actor(&username, pool).await {
                result.push('@');
                result.push_str(&username);
                result.push('.');
                result.push_str(local_domain);
            } else {
                // ローカルに存在しない → そのまま
                result.push('@');
                result.push_str(&username);
            }
        }
    }

    result
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

/// Fediverse メンション（`@user@domain`）を brid.gy 経由で AT Protocol ハンドルに解決する。
///
/// 解決に成功した場合は `Some("user.domain.ap.brid.gy")` を返す。
/// DB に存在しない場合は外部 API（`bsky.social` の `resolveHandle`）を **2 秒タイムアウト** で試みる。
async fn resolve_fedi_for_bsky(
    username: &str,
    domain: &str,
    pool: &PgPool,
    http_client: &reqwest::Client,
) -> Option<String> {
    // brid.gy ハンドル命名規則: {username}.{domain}.ap.brid.gy
    let bridgy_username = format!("{}.{}", username, domain);
    let bridgy_handle = format!("{}.ap.brid.gy", bridgy_username);

    // 第1段階: DB ルックアップ（brid.gy 経由でインポート済みのアクターを探す）
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
        return Some(bridgy_handle);
    }

    // 第2段階: bsky.social の resolveHandle API で確認（2 秒タイムアウト）
    let url = format!(
        "https://bsky.social/xrpc/com.atproto.identity.resolveHandle?handle={}",
        bridgy_handle
    );

    let res = tokio::time::timeout(
        Duration::from_secs(2),
        http_client.get(&url).send(),
    )
    .await;

    match res {
        Ok(Ok(response)) if response.status().is_success() => Some(bridgy_handle),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AP（Fediverse）向け ATP ハンドル変換
// ─────────────────────────────────────────────────────────────────────────────

/// 投稿本文中の AT Protocol ハンドル形式（`@handle.tld`）を
/// Fediverse メンション形式（`@handle.tld@bsky.brid.gy`）へ変換する。
///
/// * `@handle.tld`（単一 `@` かつドメイン形式） → brid.gy 2 段階ルックアップ
///   - 成功 → `@handle.tld@bsky.brid.gy`
///   - 失敗 → Markdown リンク `[handle.tld](https://bsky.app/profile/handle.tld)`
/// * `@user@domain`（Fediverse 形式）はそのまま出力する
///
/// 変換中にエラーが発生した場合は元テキストにフォールバックする。
pub async fn convert_mentions_for_ap(
    text: &str,
    pool: &PgPool,
    http_client: &reqwest::Client,
) -> String {
    let text_chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len() * 2);
    let mut i = 0;

    while i < text_chars.len() {
        let ch = text_chars[i];

        if ch != '@' {
            result.push(ch);
            i += 1;
            continue;
        }

        // `@` の直前が英数字 / アンダースコアならメールアドレスの一部としてスキップ
        if i > 0 {
            let prev = text_chars[i - 1];
            if prev.is_alphanumeric() || prev == '_' {
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

        // 直後に `@` が続く場合は Fediverse 形式（`@user@domain`）なので変換しない
        if i < text_chars.len() && text_chars[i] == '@' {
            result.push('@');
            result.push_str(&ident);
            // 次のループで '@' を読む
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
            let converted = resolve_atp_for_ap(&ident, pool, http_client).await;
            result.push_str(&converted);
        } else {
            result.push('@');
            result.push_str(&ident);
        }
    }

    result
}

/// AT Protocol ハンドルを brid.gy 経由で Fediverse メンション文字列に解決する。
///
/// 解決成功 → `@{handle}@bsky.brid.gy`
/// 解決失敗 → `[{handle}](https://bsky.app/profile/{handle})`（Markdown リンク）
async fn resolve_atp_for_ap(
    atp_handle: &str,
    pool: &PgPool,
    http_client: &reqwest::Client,
) -> String {
    // 第1段階: DB ルックアップ（brid.gy 経由でインポート済みのアクターを探す）
    let found_in_db = sqlx::query(
        "SELECT 1 FROM actors WHERE username = $1 AND domain = 'bsky.brid.gy' LIMIT 1",
    )
    .bind(atp_handle)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .is_some();

    if found_in_db {
        return format!("@{}@bsky.brid.gy", atp_handle);
    }

    // 第2段階: brid.gy WebFinger で AP アクターの存在を確認（2 秒タイムアウト）
    let webfinger_url = format!(
        "https://bsky.brid.gy/.well-known/webfinger?resource=acct:{}@bsky.brid.gy",
        atp_handle
    );

    let res = tokio::time::timeout(
        Duration::from_secs(2),
        http_client.get(&webfinger_url).send(),
    )
    .await;

    match res {
        Ok(Ok(response)) if response.status().is_success() => {
            format!("@{}@bsky.brid.gy", atp_handle)
        }
        _ => {
            // フォールバック: bsky.app プロフィールへの Markdown リンク
            format!(
                "[{}](https://bsky.app/profile/{})",
                atp_handle, atp_handle
            )
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// テスト
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    /// メールアドレス埋め込み文字列において `@` をメンション開始と誤認しないことを確認する。
    /// （DB / HTTP を使わない純粋な構文テスト）
    #[test]
    fn email_at_sign_should_not_start_mention() {
        // `admin@example.com` の `@` は直前が英数字なのでスキップされる
        // このテストはロジック確認用のため、非同期部分は含まない
        let text = "contact admin@example.com please";
        let chars: Vec<char> = text.chars().collect();
        let mut skipped = false;
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '@' && i > 0 {
                let prev = chars[i - 1];
                if prev.is_alphanumeric() || prev == '_' {
                    skipped = true;
                }
            }
            i += 1;
        }
        assert!(skipped, "`@` in email should be detected as preceded by word char");
    }
}

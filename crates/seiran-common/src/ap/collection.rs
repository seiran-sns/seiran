//! AP OrderedCollection の汎用ページネーション取得（フォロー中/フォロワー全件取得、#68）
//!
//! items が「アクター URI（文字列）」または「id を持つオブジェクト」であるコレクション
//! （followers/following 等）を、`first`/`next` を辿りながら取得する。Note専用の
//! `outbox::fetch_ap_history` とは異なり、item の中身を解釈せず URI 抽出のみ行う。

use super::client::ApClient;

/// コレクションを取得し、含まれる actor URI 一覧を返す。
///
/// - `max_items` に達したら打ち切る（この場合 `complete` は `false`）
/// - 非公開設定（HTTPエラー）・パース失敗はエラーとして呼び出し元を失敗させず、
///   ベストエフォートで「空 + `complete=false`」を返す
///   （Mastodon 等はフォロー/フォロワー一覧を非公開にできるため、これは異常系ではない）
/// - 戻り値: `(取得できた URI 一覧, コレクション全体を取得しきれたか)`
pub async fn fetch_ap_collection_uris(
    ap_client: &ApClient,
    collection_url: &str,
    max_items: usize,
) -> (Vec<String>, bool) {
    let mut uris = Vec::new();

    let collection: serde_json::Value = match ap_client
        .http
        .get(collection_url)
        .header("Accept", "application/activity+json, application/ld+json")
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => match r.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[ApCollection] JSONパース失敗 ({}): {}", collection_url, e);
                return (uris, false);
            }
        },
        Ok(r) => {
            tracing::info!(
                "[ApCollection] HTTP {} (非公開設定とみなしスキップ): {}",
                r.status(),
                collection_url
            );
            return (uris, false);
        }
        Err(e) => {
            tracing::error!("[ApCollection] 取得失敗（スキップ）: {}", e);
            return (uris, false);
        }
    };

    // items/orderedItems がコレクション直下にある（ページネーションなし）パターン
    if let Some(items) = collection
        .get("orderedItems")
        .or_else(|| collection.get("items"))
        .and_then(|v| v.as_array())
    {
        let hit_cap = collect_uris(items, max_items, &mut uris);
        return (uris, !hit_cap);
    }

    // ページネーションあり: first ページを処理
    let mut next_url: Option<String> = match collection.get("first") {
        Some(serde_json::Value::String(url)) => Some(url.clone()),
        Some(page_val @ serde_json::Value::Object(_)) => {
            let hit_cap = process_page(page_val, max_items, &mut uris);
            if hit_cap {
                return (uris, false);
            }
            page_val.get("next").and_then(|v| v.as_str()).map(|s| s.to_string())
        }
        // first も items も無い（空コレクション、あるいは未対応形式）
        _ => return (uris, true),
    };

    while let Some(url) = next_url {
        if uris.len() >= max_items {
            return (uris, false);
        }

        let page: serde_json::Value = match ap_client
            .http
            .get(&url)
            .header("Accept", "application/activity+json, application/ld+json")
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => match r.json().await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("[ApCollection] ページ JSONパース失敗 ({}): {}", url, e);
                    return (uris, false);
                }
            },
            Ok(r) => {
                tracing::info!("[ApCollection] ページ HTTP {} ({})", r.status(), url);
                return (uris, false);
            }
            Err(e) => {
                tracing::error!("[ApCollection] ページ取得失敗 ({}): {}", url, e);
                return (uris, false);
            }
        };

        let hit_cap = process_page(&page, max_items, &mut uris);
        if hit_cap {
            return (uris, false);
        }
        next_url = page.get("next").and_then(|v| v.as_str()).map(|s| s.to_string());
    }

    (uris, true)
}

/// ページ Value の items/orderedItems を処理して URI を追加する。
/// `max_items` に達した場合 true を返す（呼び出し側はそこで打ち切る）。
fn process_page(page: &serde_json::Value, max_items: usize, uris: &mut Vec<String>) -> bool {
    match page.get("orderedItems").or_else(|| page.get("items")).and_then(|v| v.as_array()) {
        Some(items) => collect_uris(items, max_items, uris),
        None => false,
    }
}

/// items スライスから actor URI を収集する。`max_items` に達した場合 true を返す。
fn collect_uris(items: &[serde_json::Value], max_items: usize, uris: &mut Vec<String>) -> bool {
    for item in items {
        if uris.len() >= max_items {
            return true;
        }
        if let Some(uri) = extract_uri(item) {
            uris.push(uri);
        }
    }
    false
}

/// item（アクター URI 文字列、または `id` を持つオブジェクト）から URI を抽出する。
fn extract_uri(item: &serde_json::Value) -> Option<String> {
    match item {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(_) => item.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_uri_from_string_item() {
        let item = serde_json::json!("https://example.com/users/alice");
        assert_eq!(extract_uri(&item), Some("https://example.com/users/alice".to_string()));
    }

    #[test]
    fn extract_uri_from_object_item() {
        let item = serde_json::json!({"id": "https://example.com/users/alice", "type": "Person"});
        assert_eq!(extract_uri(&item), Some("https://example.com/users/alice".to_string()));
    }

    #[test]
    fn extract_uri_returns_none_for_unsupported_shape() {
        let item = serde_json::json!(42);
        assert_eq!(extract_uri(&item), None);
    }

    #[test]
    fn collect_uris_stops_at_cap() {
        let items = vec![
            serde_json::json!("https://a.example/1"),
            serde_json::json!("https://a.example/2"),
            serde_json::json!("https://a.example/3"),
        ];
        let mut uris = Vec::new();
        let hit_cap = collect_uris(&items, 2, &mut uris);
        assert!(hit_cap);
        assert_eq!(uris, vec!["https://a.example/1", "https://a.example/2"]);
    }

    #[test]
    fn collect_uris_skips_unrecognized_items() {
        let items = vec![
            serde_json::json!("https://a.example/1"),
            serde_json::json!(null),
            serde_json::json!({"id": "https://a.example/2"}),
        ];
        let mut uris = Vec::new();
        let hit_cap = collect_uris(&items, 10, &mut uris);
        assert!(!hit_cap);
        assert_eq!(uris, vec!["https://a.example/1", "https://a.example/2"]);
    }
}

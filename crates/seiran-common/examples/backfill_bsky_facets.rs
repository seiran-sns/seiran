//! 一回限りのバックフィルスクリプト。
//!
//! `actor_history_sync`（新規フォロー時の過去ログ同期）・`upsert_bsky_post`（ピン留め同期）・
//! `persist_appview_posts`（検索結果保存）の3経路は、修正前は AppView から取得した
//! `record.facets` を無視して本文をそのまま保存していたため、URLファサード（linkファサード）が
//! Markdownリンクに変換されず、seiran上でリンク表示されない投稿が残ってしまっていた。
//!
//! 対象の Bsky 由来投稿（at_uri あり・削除されていない）について、PDS の
//! `com.atproto.repo.getRecord` から現在のレコードを再取得し、facets が本文と食い違いなく
//! 適用できる場合のみ body / mention_facets を更新する。
//!
//! 実行方法:
//!   DATABASE_URL=postgres://... cargo run -p seiran-common --example backfill_bsky_facets -- --dry-run
//!   DATABASE_URL=postgres://... cargo run -p seiran-common --example backfill_bsky_facets
use std::collections::HashMap;

use seiran_common::atp::{apply_bsky_facets, ParsedFacet};

#[derive(Debug, sqlx::FromRow)]
struct PostRow {
    id: i64,
    at_uri: String,
    body: String,
}

#[tokio::main]
async fn main() {
    let dry_run = std::env::args().any(|a| a == "--dry-run");
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("DB接続失敗");

    let rows: Vec<PostRow> = sqlx::query_as(
        "SELECT id, at_uri, body FROM posts
         WHERE at_uri IS NOT NULL AND deleted_at IS NULL AND is_local = false
         ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("posts取得失敗");

    println!("対象候補: {}件 (dry_run={})", rows.len(), dry_run);

    let client = reqwest::Client::new();
    let mut updated = 0usize;
    let mut skipped_mismatch = 0usize;
    let mut skipped_no_record = 0usize;
    let mut unchanged = 0usize;

    for chunk in rows.chunks(25) {
        let by_uri: HashMap<&str, &PostRow> =
            chunk.iter().map(|r| (r.at_uri.as_str(), r)).collect();

        let uri_params: String = chunk
            .iter()
            .map(|r| format!("uris={}", urlencoding::encode(&r.at_uri)))
            .collect::<Vec<_>>()
            .join("&");
        let url = format!(
            "https://api.bsky.app/xrpc/app.bsky.feed.getPosts?{}",
            uri_params
        );

        let resp = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("getPosts HTTPエラー（チャンクskip）: {}", e);
                continue;
            }
        };
        if !resp.status().is_success() {
            eprintln!(
                "getPosts 非成功ステータス {}（チャンクskip）",
                resp.status()
            );
            continue;
        }
        let json: serde_json::Value = match resp.json().await {
            Ok(j) => j,
            Err(e) => {
                eprintln!("getPosts JSONパース失敗（チャンクskip）: {}", e);
                continue;
            }
        };
        let posts = match json["posts"].as_array() {
            Some(a) => a,
            None => continue,
        };

        let mut seen_uris = std::collections::HashSet::new();
        for p in posts {
            let Some(uri) = p["uri"].as_str() else {
                continue;
            };
            seen_uris.insert(uri.to_string());
            let Some(row) = by_uri.get(uri) else {
                continue;
            };

            let record = &p["record"];
            let record_text = record["text"].as_str().unwrap_or("");
            let facets = record.get("facets").cloned();
            let Some(facets_value) = facets else {
                continue;
            };
            let Some(facets_array) = facets_value.as_array() else {
                continue;
            };
            if facets_array.is_empty() {
                continue;
            }

            // 現在保存されている body が PDS 側の元テキストと一致しない場合、facet の
            // byteStart/byteEnd を機械的に適用するとズレる恐れがあるため触らずスキップする。
            if row.body != record_text {
                skipped_mismatch += 1;
                eprintln!("本文不一致でskip id={} uri={}", row.id, row.at_uri);
                continue;
            }

            let parsed_facets: Vec<ParsedFacet> = match serde_json::from_value(facets_value.clone())
            {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("facetsパース失敗 id={}: {}", row.id, e);
                    continue;
                }
            };

            let (new_body, mention_facets) = apply_bsky_facets(record_text, parsed_facets);

            if new_body == row.body {
                unchanged += 1;
                continue;
            }

            println!(
                "id={} uri={}\n  旧: {:?}\n  新: {:?}",
                row.id, row.at_uri, row.body, new_body
            );

            if !dry_run {
                sqlx::query("UPDATE posts SET body = $1, mention_facets = $2 WHERE id = $3")
                    .bind(&new_body)
                    .bind(&mention_facets)
                    .bind(row.id)
                    .execute(&pool)
                    .await
                    .expect("UPDATE失敗");
            }
            updated += 1;
        }

        for row in chunk {
            if !seen_uris.contains(&row.at_uri) {
                skipped_no_record += 1;
            }
        }
    }

    println!(
        "完了: 更新{}件 / 変更なし{}件 / 本文不一致skip{}件 / レコード取得不可skip{}件",
        updated, unchanged, skipped_mismatch, skipped_no_record
    );
}

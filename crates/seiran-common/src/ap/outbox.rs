//! ActivityPub Outbox フェッチ & 過去ログ取得モジュール
//!
//! リモートアクターの Outbox コレクションをページネーションしながらフェッチし、
//! 「過去30日間 / 最大300件」のキャップを適用して Note を返す。

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::client::{ApClient, ApError};
use crate::jobs::inbound_activity_process::strip_html;

/// AP Note（投稿）型
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ApNote {
    pub id: String,
    #[serde(rename = "type")]
    pub note_type: String,
    pub content: Option<String>,
    pub published: Option<String>,
    #[serde(rename = "attributedTo")]
    pub attributed_to: Option<String>,
    pub url: Option<String>,
    #[serde(rename = "inReplyTo")]
    pub in_reply_to: Option<String>,
    /// seiran 拡張フィールド: 他 seiran サーバー間のマージ用共通 UUID
    #[serde(rename = "seiranPostUuid")]
    pub seiran_post_uuid: Option<String>,
}

/// 指定アクターの AP Outbox から過去ログを取得する
///
/// - 最大 `max_posts` 件かつ `max_days` 日前までを対象（どちらか早い方で停止）
/// - outbox 非公開・取得失敗の場合はベストエフォートで空 Vec を返す
pub async fn fetch_ap_history(
    ap_client: &ApClient,
    actor_uri: &str,
    max_posts: usize,
    max_days: i64,
) -> Result<Vec<ApNote>, ApError> {
    let actor = ap_client.fetch_actor(actor_uri).await?;
    let outbox_url = match actor.outbox {
        Some(url) => url,
        None => {
            tracing::warn!("[ApOutbox] {} の outbox フィールドが存在しません（スキップ）", actor_uri);
            return Ok(vec![]);
        }
    };

    let since = Utc::now() - Duration::days(max_days);
    let mut notes: Vec<ApNote> = Vec::new();

    // Outbox コレクション取得
    let collection: serde_json::Value = match ap_client.http
        .get(&outbox_url)
        .header("Accept", "application/activity+json, application/ld+json")
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => match r.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[ApOutbox] Outbox JSONパース失敗: {}", e);
                return Ok(vec![]);
            }
        },
        Ok(r) => {
            tracing::warn!("[ApOutbox] Outbox HTTP {} (非公開とみなしスキップ): {}", r.status(), outbox_url);
            return Ok(vec![]);
        }
        Err(e) => {
            tracing::error!("[ApOutbox] Outbox 取得失敗 (スキップ): {}", e);
            return Ok(vec![]);
        }
    };

    // orderedItems がコレクション直下にある（ページネーションなし）パターン
    if let Some(items) = collection.get("orderedItems").and_then(|v| v.as_array()) {
        collect_notes(items, &since, max_posts, &mut notes);
        return Ok(notes);
    }

    // ページネーションあり: first ページを処理
    let mut next_url: Option<String> = match collection.get("first") {
        Some(serde_json::Value::String(url)) => Some(url.clone()),
        Some(page_val @ serde_json::Value::Object(_)) => {
            let done = process_page(page_val, &since, max_posts, &mut notes);
            if done {
                return Ok(notes);
            }
            page_val.get("next").and_then(|v| v.as_str()).map(|s| s.to_string())
        }
        _ => return Ok(notes),
    };

    // ページネーションループ
    while let Some(url) = next_url {
        if notes.len() >= max_posts {
            break;
        }

        let page: serde_json::Value = match ap_client.http
            .get(&url)
            .header("Accept", "application/activity+json, application/ld+json")
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => match r.json().await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("[ApOutbox] ページ JSONパース失敗 ({}): {}", url, e);
                    break;
                }
            },
            Ok(r) => {
                tracing::info!("[ApOutbox] ページ HTTP {} ({})", r.status(), url);
                break;
            }
            Err(e) => {
                tracing::error!("[ApOutbox] ページ取得失敗 ({}): {}", url, e);
                break;
            }
        };

        let done = process_page(&page, &since, max_posts, &mut notes);
        if done {
            break;
        }
        next_url = page.get("next").and_then(|v| v.as_str()).map(|s| s.to_string());
    }

    Ok(notes)
}

/// ページ Value の orderedItems を処理してノートを追加する
/// 終了条件（件数/日付上限）に達した場合 true を返す
fn process_page(
    page: &serde_json::Value,
    since: &DateTime<Utc>,
    max_posts: usize,
    notes: &mut Vec<ApNote>,
) -> bool {
    match page.get("orderedItems").and_then(|v| v.as_array()) {
        Some(items) => collect_notes(items, since, max_posts, notes),
        None => false,
    }
}

/// items スライスからノートを収集する
/// 終了条件に達した場合 true を返す
fn collect_notes(
    items: &[serde_json::Value],
    since: &DateTime<Utc>,
    max_posts: usize,
    notes: &mut Vec<ApNote>,
) -> bool {
    for item in items {
        if notes.len() >= max_posts {
            return true;
        }
        if let Some(note) = extract_create_note(item) {
            if note
                .published
                .as_deref()
                .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                .map(|t| t < *since)
                .unwrap_or(false)
            {
                return true;
            }
            notes.push(note);
        }
    }
    false
}

/// 指定アクターの featured collection（ピン留め投稿, #61）を取得する。
/// Actor に `featured` フィールドが無い場合や取得・パースに失敗した場合は
/// ベストエフォートで空 Vec を返す（プロフィール表示自体は失敗させない）。
pub async fn fetch_ap_featured(ap_client: &ApClient, actor_uri: &str) -> Result<Vec<ApNote>, ApError> {
    let actor = ap_client.fetch_actor(actor_uri).await?;
    let featured_url = match actor.featured {
        Some(url) => url,
        None => return Ok(vec![]),
    };

    let collection: serde_json::Value = match ap_client
        .http
        .get(&featured_url)
        .header("Accept", "application/activity+json, application/ld+json")
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => match r.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[ApFeatured] JSONパース失敗: {}", e);
                return Ok(vec![]);
            }
        },
        Ok(r) => {
            tracing::info!("[ApFeatured] HTTP {} ({})", r.status(), featured_url);
            return Ok(vec![]);
        }
        Err(e) => {
            tracing::error!("[ApFeatured] 取得失敗（スキップ）: {}", e);
            return Ok(vec![]);
        }
    };

    let items = collection
        .get("orderedItems")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(items.iter().filter_map(extract_note_flexible).collect())
}

/// featured collection の要素から Note を抽出する。`type: "Note"` を直接含む実装
/// （Mastodon 等の慣習）と `type: "Create"` でラップされた実装の両方に対応する。
fn extract_note_flexible(value: &serde_json::Value) -> Option<ApNote> {
    let item_type = value.get("type")?.as_str()?;
    let obj = if item_type == "Create" {
        value.get("object")?
    } else {
        value
    };
    if obj.is_string() {
        return None;
    }
    let obj_type = obj.get("type")?.as_str()?;
    if obj_type != "Note" && obj_type != "Article" {
        return None;
    }

    Some(ApNote {
        id: obj.get("id")?.as_str()?.to_string(),
        note_type: obj_type.to_string(),
        content: obj.get("content").and_then(|v| v.as_str()).map(|s| s.to_string()),
        published: obj.get("published").and_then(|v| v.as_str()).map(|s| s.to_string()),
        attributed_to: obj.get("attributedTo").and_then(|v| v.as_str()).map(|s| s.to_string()),
        url: obj.get("url").and_then(|v| v.as_str()).map(|s| s.to_string()),
        in_reply_to: obj.get("inReplyTo").and_then(|v| v.as_str()).map(|s| s.to_string()),
        seiran_post_uuid: obj.get("seiranPostUuid").and_then(|v| v.as_str()).map(|s| s.to_string()),
    })
}

/// AP Note を `posts` テーブルへ反映し、ローカル post_id を返す（既存なら既存の id、
/// 無ければ新規挿入）。リモートアクターのピン留め（featured collection）同期専用（#61）。
pub async fn upsert_ap_note(
    pool: &sqlx::PgPool,
    actor_id: i64,
    note: &ApNote,
) -> Result<i64, sqlx::Error> {
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT id FROM posts WHERE ap_object_id = $1 LIMIT 1")
        .bind(&note.id)
        .fetch_optional(pool)
        .await?
    {
        return Ok(id);
    }

    let created_at = note
        .published
        .as_deref()
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        .unwrap_or_else(Utc::now);
    let post_id = crate::generate_snowflake_id(created_at);
    // AP Note の content は HTML（Mastodon 等は <p>/<a> 等でラップして送る）のため、
    // 他の受信経路（handle_create_note）と同じく strip_html でプレーンテキスト化する。
    let body = strip_html(&note.content.clone().unwrap_or_default());

    sqlx::query(
        "INSERT INTO posts (id, actor_id, body, ap_object_id, seiran_post_uuid, created_at)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (ap_object_id) DO NOTHING",
    )
    .bind(post_id)
    .bind(actor_id)
    .bind(&body)
    .bind(&note.id)
    .bind(note.seiran_post_uuid.as_deref())
    .bind(created_at)
    .execute(pool)
    .await?;

    // ON CONFLICT で INSERT がスキップされた場合（並行同期の競合）に備え、確定した id を引き直す。
    sqlx::query_scalar::<_, i64>("SELECT id FROM posts WHERE ap_object_id = $1 LIMIT 1")
        .bind(&note.id)
        .fetch_one(pool)
        .await
}

/// Create アクティビティ Value から Note を抽出する
fn extract_create_note(value: &serde_json::Value) -> Option<ApNote> {
    let activity_type = value.get("type")?.as_str()?;
    if activity_type != "Create" {
        return None;
    }

    let obj = value.get("object")?;
    if obj.is_string() {
        // object が URL 文字列のみ → 別途フェッチが必要（Phase 4.1 ではスキップ）
        return None;
    }

    let obj_type = obj.get("type")?.as_str()?;
    if obj_type != "Note" && obj_type != "Article" {
        return None;
    }

    Some(ApNote {
        id: obj.get("id")?.as_str()?.to_string(),
        note_type: obj_type.to_string(),
        content: obj.get("content").and_then(|v| v.as_str()).map(|s| s.to_string()),
        published: obj.get("published").and_then(|v| v.as_str()).map(|s| s.to_string()),
        attributed_to: obj.get("attributedTo").and_then(|v| v.as_str()).map(|s| s.to_string()),
        url: obj.get("url").and_then(|v| v.as_str()).map(|s| s.to_string()),
        in_reply_to: obj.get("inReplyTo").and_then(|v| v.as_str()).map(|s| s.to_string()),
        seiran_post_uuid: obj.get("seiranPostUuid").and_then(|v| v.as_str()).map(|s| s.to_string()),
    })
}

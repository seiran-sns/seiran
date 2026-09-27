use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use seiran_common::version::SERVER_VERSION;
use std::sync::Arc;

use crate::AppState;

/// HTML タグを取り除く（`crates/seiran-api/src/handlers/notes/validation.rs` の
/// 同名関数と同等。別crateのためここでは軽量な独自実装を持つ）。
fn strip_html_tags(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

pub async fn nodeinfo_discovery_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let body = serde_json::json!({
        "links": [{
            "rel": "http://nodeinfo.diaspora.software/ns/schema/2.1",
            "href": format!("https://{}/nodeinfo/2.1", state.local_domain)
        }]
    });
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        Json(body),
    )
        .into_response()
}

pub async fn nodeinfo_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let (user_count, post_count) = seiran_common::repository::ap_public::nodeinfo_counts(&state.db)
        .await
        .unwrap_or((0, 0));

    // サイト外観を Misskey 系 nodeinfo の慣習（nodeName / themeColor / iconUrl）で同梱する。
    let appearance: std::collections::HashMap<String, String> =
        seiran_common::repository::ap_public::site_settings_values(
            &state.db,
            &[
                "site_name",
                "site_color",
                "site_icon_url",
                "site_description",
            ],
        )
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    let get = |k: &str| appearance.get(k).cloned().unwrap_or_default();
    // site_name はHTML可（#243、ログイン画面のサイトタイトル表示用）。nodeName はHTML想定
    // でないため、タグを除去したプレーンテキストを使う。
    let site_name = {
        let n = get("site_name");
        let n = if n.is_empty() {
            "seiran".to_string()
        } else {
            n
        };
        strip_html_tags(&n)
    };
    let site_color = get("site_color");
    let site_icon_url = get("site_icon_url");
    // site_description もHTML可（#243、ログイン画面用）。nodeDescription はHTML想定でないため
    // タグを除去したプレーンテキストを使う。
    let site_description = strip_html_tags(&get("site_description"));

    let mut metadata = serde_json::Map::new();
    metadata.insert("nodeName".into(), serde_json::json!(site_name));
    if !site_color.is_empty() {
        metadata.insert("themeColor".into(), serde_json::json!(site_color));
    }
    if !site_icon_url.is_empty() {
        metadata.insert("iconUrl".into(), serde_json::json!(site_icon_url));
    }
    if !site_description.is_empty() {
        metadata.insert(
            "nodeDescription".into(),
            serde_json::json!(site_description),
        );
    }
    // kmyblue（Mastodonフォーク）は既知softwareリストに無いインスタンスに対し、
    // ここに "emoji_reaction" が含まれるかどうかでカスタム絵文字リアクション対応を判定する。
    metadata.insert("features".into(), serde_json::json!(["emoji_reaction"]));

    let body = serde_json::json!({
        "version": "2.1",
        "software": {
            "name": "seiran",
            "version": SERVER_VERSION
        },
        "protocols": ["activitypub"],
        "usage": {
            "users": {
                "total": user_count,
                "activeMonth": user_count,
                "activeHalfyear": user_count
            },
            "localPosts": post_count
        },
        "openRegistrations": true,
        "metadata": metadata
    });

    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; profile=\"http://nodeinfo.diaspora.software/ns/schema/2.1#\"",
        )],
        Json(body),
    )
        .into_response()
}

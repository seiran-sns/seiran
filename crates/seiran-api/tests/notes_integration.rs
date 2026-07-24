//! notes ハンドラの結合テスト（実 DB 使用）。
//!
//! 実行方法:
//! ```sh
//! cargo test -p seiran-api --test notes_integration -- --ignored
//! ```
//! 事前に `seiran1`（パスワード `seiranda`）がローカル DB に存在すること
//! （CLAUDE.md の規約：無ければ `/api/setup` や `/api/auth/register` で作成してよい）。

mod support;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use support::{authed_json_request, body_json, login_test_user, test_router};

use seiran_common::get_db_pool;

/// 投稿作成 → 取得の往復が一致することを確認する。
/// `deliver_to_fedi`/`deliver_to_bsky` を `false` にして実際の連合配送・ATP コミットを
/// 起こさない（enqueue はされるがテストプロセスに Worker はいないため実害はない）。
#[tokio::test]
#[ignore = "実DB（DATABASE_URL）と既存の seiran1 ユーザーが必要"]
async fn create_note_and_fetch_round_trip() {
    let app = test_router().await;
    let token = login_test_user(&app, "seiran1").await;

    let text = format!("結合テスト投稿 {}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
    let create_req = authed_json_request(
        "POST",
        "/api/notes/create",
        &token,
        serde_json::json!({
            "text": text,
            "deliver_to_fedi": false,
            "deliver_to_bsky": false,
        }),
    );
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::OK);
    let created = body_json(create_res).await;
    assert_eq!(created["text"], text);
    let note_id = created["id"].as_str().expect("id フィールドがありません").to_string();

    let get_req = Request::builder()
        .method("GET")
        .uri(format!("/api/notes/{}", note_id))
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let get_res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let fetched = body_json(get_res).await;
    assert_eq!(fetched["id"], note_id);
    assert_eq!(fetched["text"], text);
}

/// 未認証で POST /api/notes/create を叩くと、401 + JSON ボディ（ApiError 形式）が返る。
/// `AuthedUser` extractor が生タプルではなく必ず ApiError の JSON を返すことの回帰テスト
/// （2026-07リファクタリング以前は一部ハンドラが素のテキストボディを返しており、
/// フロントエンドの `res.json()` がパースに失敗する latent バグがあった）。
#[tokio::test]
#[ignore = "実DB（DATABASE_URL）が必要"]
async fn create_note_without_auth_returns_json_401() {
    let app = test_router().await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/notes/create")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::json!({"text": "x"}).to_string()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    let json = body_json(res).await;
    assert!(json["code"].is_string(), "ApiError の JSON ボディでなければならない: {:?}", json);
}

/// 存在しない note_id への GET は 404 + JSON を返す。
#[tokio::test]
#[ignore = "実DB（DATABASE_URL）が必要"]
async fn get_note_not_found_returns_json_404() {
    let app = test_router().await;

    let req = Request::builder()
        .method("GET")
        .uri("/api/notes/999999999999999999")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let json = body_json(res).await;
    assert!(json["code"].is_string(), "ApiError の JSON ボディでなければならない: {:?}", json);
}

/// リポストの `GET /api/notes/:id` レスポンスで、埋め込まれた元ポスト（`renote`）の
/// `emojis` マップが失われないことの回帰テスト（#77）。`embed_renotes`（`queries.rs`）の
/// SELECT文が `emoji_map` を取得していなかったため、リポスト経由で見る本文中の
/// カスタム絵文字ショートコードが画像化されず「展開されなくなった」ように見えるバグがあった。
#[tokio::test]
#[ignore = "実DB（DATABASE_URL）と既存の seiran1 ユーザーが必要"]
async fn embed_renotes_preserves_original_post_emoji_map() {
    let app = test_router().await;
    let token = login_test_user(&app, "seiran1").await;

    let text = format!("絵文字埋め込みテスト元投稿 {}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
    let create_req = authed_json_request(
        "POST",
        "/api/notes/create",
        &token,
        serde_json::json!({ "text": text, "deliver_to_fedi": false, "deliver_to_bsky": false }),
    );
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::OK);
    let original = body_json(create_res).await;
    let original_id: i64 = original["id"].as_str().unwrap().parse().unwrap();

    // Fedi受信時にのみ書き込まれる`emoji_map`（本テストでは受信フローを再現せず直接設定する）。
    let pool = get_db_pool().await.expect("DB接続に失敗");
    sqlx::query("UPDATE posts SET emoji_map = $1 WHERE id = $2")
        .bind(serde_json::json!({":test_emoji:": "https://example.com/test.png"}))
        .bind(original_id)
        .execute(&pool)
        .await
        .expect("emoji_map更新に失敗");

    let repost_req = authed_json_request(
        "POST",
        "/api/notes/create",
        &token,
        serde_json::json!({ "renote_id": original_id.to_string(), "deliver_to_fedi": false, "deliver_to_bsky": false }),
    );
    let repost_res = app.clone().oneshot(repost_req).await.unwrap();
    assert_eq!(repost_res.status(), StatusCode::OK);
    let repost = body_json(repost_res).await;
    let repost_id = repost["id"].as_str().unwrap().to_string();

    let get_req = Request::builder()
        .method("GET")
        .uri(format!("/api/notes/{}", repost_id))
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let get_res = app.oneshot(get_req).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let fetched = body_json(get_res).await;

    assert_eq!(
        fetched["renote"]["emojis"][":test_emoji:"],
        "https://example.com/test.png",
        "埋め込まれた元ポストのemojisマップが失われている: {:?}",
        fetched["renote"]
    );
}

/// ローカル投稿の本文中に含まれる `:shortcode:` が、既存の `custom_emojis` と照合されて
/// `emojis` マップに自動で解決されることの回帰テスト（#77）。`create_regular_post` が
/// ローカル投稿作成時に本文からショートコードを抽出せず、`emoji_map` を常に空のまま
/// INSERT していたため、Fedi経由の受信投稿と異なりローカル投稿では絵文字ショートコードが
/// リポスト有無に関わらず一切画像化されないバグがあった。
#[tokio::test]
#[ignore = "実DB（DATABASE_URL）と既存の seiran1 ユーザーが必要"]
async fn create_note_resolves_local_custom_emoji_shortcode_in_body() {
    let pool = get_db_pool().await.expect("DB接続に失敗");

    let shortcode = format!("test_emoji_{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
    let media_file_id = seiran_common::generate_snowflake_id(chrono::Utc::now());
    let emoji_id = seiran_common::generate_snowflake_id(chrono::Utc::now());

    sqlx::query(
        "INSERT INTO media_files (id, storage_provider_id, sha256, size, storage_key)
         VALUES ($1, 1, $2, 1, $3)",
    )
    .bind(media_file_id)
    .bind(format!("{:0>64}", media_file_id)) // sha256 は char(64) 制約のためダミー値を桁埋め
    .bind(format!("test/{}.png", shortcode))
    .execute(&pool)
    .await
    .expect("media_files INSERT失敗");

    sqlx::query("INSERT INTO custom_emojis (id, shortcode, media_file_id) VALUES ($1, $2, $3)")
        .bind(emoji_id)
        .bind(&shortcode)
        .bind(media_file_id)
        .execute(&pool)
        .await
        .expect("custom_emojis INSERT失敗");

    let app = test_router().await;
    let token = login_test_user(&app, "seiran1").await;

    let text = format!("絵文字テスト :{}: 本文", shortcode);
    let create_req = authed_json_request(
        "POST",
        "/api/notes/create",
        &token,
        serde_json::json!({ "text": text, "deliver_to_fedi": false, "deliver_to_bsky": false }),
    );
    let create_res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_res.status(), StatusCode::OK);
    let created = body_json(create_res).await;

    let expected_key = format!(":{}:", shortcode);
    assert!(
        created["emojis"][&expected_key].as_str().is_some(),
        "投稿作成レスポンスの emojis に本文中のショートコードが解決されていない: {:?}",
        created["emojis"]
    );

    let post_id = created["id"].as_str().unwrap().to_string();
    let get_req = Request::builder()
        .method("GET")
        .uri(format!("/api/notes/{}", post_id))
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let get_res = app.oneshot(get_req).await.unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let fetched = body_json(get_res).await;
    assert!(
        fetched["emojis"][&expected_key].as_str().is_some(),
        "GET /api/notes/:id の emojis に本文中のショートコードが解決されていない: {:?}",
        fetched["emojis"]
    );

    // クリーンアップ（テスト用に作成した絵文字・メディアファイルを削除）。
    sqlx::query("DELETE FROM custom_emojis WHERE id = $1").bind(emoji_id).execute(&pool).await.ok();
    sqlx::query("DELETE FROM media_files WHERE id = $1").bind(media_file_id).execute(&pool).await.ok();
}

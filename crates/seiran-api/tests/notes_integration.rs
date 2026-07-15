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

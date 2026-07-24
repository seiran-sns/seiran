//! アカウント設定のメールアドレス変更（#59）結合テスト（実 DB 使用）。
//!
//! `request-change` は実際に SMTP 経由でメール送信を行うため、誤送信を避けて
//! ここではテストしない。`email_changes` テーブルへの INSERT を直接行うことで
//! `confirm-change` （トークン消費 → `users.email` 更新）側のみを検証する。
//!
//! 既存の `seiran1` 等を汚さないよう、テストごとに使い捨てユーザーを新規登録して使う。
//!
//! 実行方法:
//! ```sh
//! cargo test -p seiran-api --test account_email_change_integration -- --ignored
//! ```

mod support;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use seiran_common::{generate_snowflake_id, get_db_pool};
use tower::ServiceExt;

use support::{authed_json_request, body_json, test_router};

/// テスト専用の使い捨てユーザーを登録し、(user_id, token) を返す。
/// このDBは `require_email_verification=true` のため、`email_verifications` に直接
/// INSERT してトークンを取得し `registration_token` 経由で登録する（メール送信を経由しない）。
async fn register_test_user(app: &axum::Router, pool: &sqlx::PgPool, username_prefix: &str) -> (i64, String) {
    let suffix = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let username = format!("{}{}", username_prefix, suffix);
    let email = format!("{}@example.com", username);

    let ev_id = generate_snowflake_id(chrono::Utc::now());
    let ev_row: (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO email_verifications (id, email) VALUES ($1, $2) RETURNING token",
    )
    .bind(ev_id)
    .bind(&email)
    .fetch_one(pool)
    .await
    .expect("email_verifications への INSERT に失敗");

    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/register")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "username": username,
                "password": "seiranda-test",
                "registration_token": ev_row.0.to_string(),
            })
            .to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "テストユーザー登録に失敗");
    let body = body_json(res).await;
    let token = body["token"].as_str().expect("token フィールドがありません").to_string();
    let user_id = body["user"]["id"].as_i64().expect("user.id は数値のはず");
    (user_id, token)
}

/// email_changes に直接 INSERT してトークンを発行する（request-change のメール送信を経由しない）。
async fn insert_email_change(pool: &sqlx::PgPool, user_id: i64, new_email: &str) -> uuid::Uuid {
    let id = generate_snowflake_id(chrono::Utc::now());
    let row: (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO email_changes (id, user_id, new_email) VALUES ($1, $2, $3) RETURNING token",
    )
    .bind(id)
    .bind(user_id)
    .bind(new_email)
    .fetch_one(pool)
    .await
    .expect("email_changes への INSERT に失敗");
    row.0
}

/// 有効なトークンを confirm-change に渡すと users.email が更新され、
/// トークンが消費済み（再利用不可）になることを確認する。
#[tokio::test]
#[ignore = "実DB（DATABASE_URL）が必要"]
async fn confirm_email_change_updates_email_and_consumes_token() {
    let app = test_router().await;
    let pool = get_db_pool().await.expect("DB接続に失敗");
    let (user_id, token) = register_test_user(&app, &pool, "e2eemailchg").await;

    let new_email = format!(
        "e2e-email-change-{}@example.com",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let change_token = insert_email_change(&pool, user_id, &new_email).await;

    let confirm_req = authed_json_request(
        "POST",
        "/api/account/email/confirm-change",
        &token,
        serde_json::json!({ "token": change_token.to_string() }),
    );
    let confirm_res = app.clone().oneshot(confirm_req).await.unwrap();
    assert_eq!(confirm_res.status(), StatusCode::OK);

    let updated_email: (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .expect("email の取得に失敗");
    assert_eq!(updated_email.0, new_email);

    // トークンは使い捨てなので再利用は失敗する。
    let replay_req = authed_json_request(
        "POST",
        "/api/account/email/confirm-change",
        &token,
        serde_json::json!({ "token": change_token.to_string() }),
    );
    let replay_res = app.clone().oneshot(replay_req).await.unwrap();
    assert_eq!(replay_res.status(), StatusCode::BAD_REQUEST);
}

/// 存在しない・期限切れのトークンは 400 INVALID_TOKEN を返す。
#[tokio::test]
#[ignore = "実DB（DATABASE_URL）が必要"]
async fn confirm_email_change_rejects_invalid_token() {
    let app = test_router().await;
    let pool = get_db_pool().await.expect("DB接続に失敗");
    let (_user_id, token) = register_test_user(&app, &pool, "e2eemailchg").await;

    let confirm_req = authed_json_request(
        "POST",
        "/api/account/email/confirm-change",
        &token,
        serde_json::json!({ "token": uuid::Uuid::new_v4().to_string() }),
    );
    let confirm_res = app.clone().oneshot(confirm_req).await.unwrap();
    assert_eq!(confirm_res.status(), StatusCode::BAD_REQUEST);
}

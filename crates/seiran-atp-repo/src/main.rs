//! seiran-atp-repo — AT Protocol Firehose リスナー
//!
//! Bluesky Firehose に WebSocket で接続し、フォロー済みアクターの新規ポストを
//! リアルタイムで DB に保存する常駐プロセス。
//!
//! # 環境変数
//! - `DATABASE_URL` — PostgreSQL 接続 URL

mod firehose;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let db_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL 環境変数が設定されていません");

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .expect("DB 接続に失敗しました");

    eprintln!("[seiran-atp-repo] Firehose リスナーを起動します。");

    firehose::run(pool).await;
}

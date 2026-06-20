use sqlx::{postgres::PgPoolOptions, PgPool};
use std::env;

/// データベース接続プールを初期化して取得します。
/// 環境変数 `DATABASE_URL` から取得し、デフォルトはローカルのDockerコンテナに接続します。
pub async fn get_db_pool() -> Result<PgPool, sqlx::Error> {
    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgrespassword@localhost:5432/seiran".to_string());
    
    PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
}

/// 共通のマイグレーション（SQLファイル群）をデータベースに適用します。
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

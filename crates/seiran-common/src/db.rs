use sqlx::{postgres::PgPoolOptions, PgPool};
use std::env;
use std::time::Duration;

/// データベース接続プールを初期化して取得します。
/// 環境変数 `DATABASE_URL` から取得し、デフォルトはローカルのDockerコンテナに接続します。
/// 最大接続数は `DB_MAX_CONNECTIONS`（既定10）で調整できる。split-role構成（api /
/// federation-inbox / worker が別プロセス）ではプロセスごとにこの上限を持つため、
/// 合計接続数はロール数倍になる点に注意。
/// `acquire_timeout` はsqlx既定の30秒だとプール枯渇時にリクエストが長時間ハングしてから
/// 500を返すため、5秒に短縮して早期に失敗を返す（観測しやすくするため）。
pub async fn get_db_pool() -> Result<PgPool, sqlx::Error> {
    let database_url = env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgres://postgres:postgrespassword@localhost:5432/seiran".to_string()
    });
    let max_connections: u32 = env::var("DB_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url)
        .await
}

/// 共通のマイグレーション（SQLファイル群）をデータベースに適用します。
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

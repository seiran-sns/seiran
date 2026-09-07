use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use std::env;
use std::time::Duration;

/// `POSTGRES_USER`/`POSTGRES_PASSWORD`/`POSTGRES_DB`/`DB_HOST`/`DB_PORT` から接続先を
/// 組み立てる（`DATABASE_URL`という完成済みURL文字列は持たない。値を1箇所ずつしか
/// 持たないことで、パスワード変更時などに複数箇所を同期させる必要をなくすため）。
/// `DB_HOST`/`DB_PORT`省略時はネイティブ開発を前提に`localhost`/`5432`にフォールバックする
/// （Docker運用では`docker-compose.yml`が`DB_HOST=db`を渡す）。
fn connect_options() -> PgConnectOptions {
    let user = env::var("POSTGRES_USER").unwrap_or_else(|_| "postgres".to_string());
    let password = env::var("POSTGRES_PASSWORD").unwrap_or_else(|_| "postgrespassword".to_string());
    let database = env::var("POSTGRES_DB").unwrap_or_else(|_| "seiran".to_string());
    let host = env::var("DB_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port: u16 = env::var("DB_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5432);

    PgConnectOptions::new()
        .host(&host)
        .port(port)
        .username(&user)
        .password(&password)
        .database(&database)
}

/// このプロセス内でDBを同時に使いうる既知の並列コンポーネント数から、余裕を持った
/// プール上限を見積もる。`extra_concurrent_tasks`には呼び出し元（`seiran-server`）が
/// 把握しているロール固有の並列度（例: 埋め込みworkerの`WorkerEngine::max_concurrent`＝
/// `DEFAULT_MAX_CONCURRENT_JOBS`）を渡す。
///
/// 内訳: `available_parallelism()`（HTTPハンドラの同時処理見積もり。axum/tokioのHTTP
/// リクエスト処理自体には上限を設けていないため、tokioのデフォルトワーカースレッド数＝
/// CPUコア数で近似する） + `extra_concurrent_tasks` + `FIXED_BUFFER`（Jetstream/
/// BskyDmPoll等の常駐バックグラウンドタスクや突発的な同時アクセスの余裕分）。
///
/// PostgreSQL既定の`max_connections`（100）を単一ロールの見積もりだけで圧迫しないよう
/// `MAX_RECOMMENDED`で上限クランプする。split-role構成では複数プロセスがそれぞれこの
/// 値を持つため、合計接続数はロール数倍になる点に注意（`get_db_pool`のドキュメント参照）。
const FIXED_BUFFER: u32 = 5;
const MAX_RECOMMENDED_CONNECTIONS: u32 = 50;

pub fn recommended_max_connections(extra_concurrent_tasks: u32) -> u32 {
    let cpu_estimate = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);
    (cpu_estimate + extra_concurrent_tasks + FIXED_BUFFER).min(MAX_RECOMMENDED_CONNECTIONS)
}

/// データベース接続プールを初期化して取得します。接続先の組み立ては`connect_options`参照。
/// 最大接続数は環境変数 `DB_MAX_CONNECTIONS` を明示指定していれば常にそれを優先し、
/// 未設定なら呼び出し元が渡す`default_max_connections`（`recommended_max_connections`
/// 参照）を使う。split-role構成（api / federation-inbox / worker が別プロセス）では
/// プロセスごとにこの上限を持つため、合計接続数はロール数倍になる点に注意。
/// `acquire_timeout` はsqlx既定の30秒だとプール枯渇時にリクエストが長時間ハングしてから
/// 500を返すため、5秒に短縮して早期に失敗を返す（観測しやすくするため）。
pub async fn get_db_pool(default_max_connections: u32) -> Result<PgPool, sqlx::Error> {
    let max_connections: u32 = env::var("DB_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default_max_connections);

    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(connect_options())
        .await
}

/// 共通のマイグレーション（SQLファイル群）をデータベースに適用します。
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

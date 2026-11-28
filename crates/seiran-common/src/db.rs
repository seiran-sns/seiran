use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgPool};
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

/// マイグレーションを直列化する advisory lock のキー（`pg_try_advisory_lock(int4, int4)`）。
/// 2引数形式は1引数（bigint）形式のキー空間と重ならず、第1キーは `leader` のものと別にする。
const MIGRATION_LOCK_KEY: (i32, i32) = (0x5345_4d47, 1);
/// マイグレーションの担当が取れなかったときに取り直すまでの間隔。
const MIGRATION_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(500);

/// 共通のマイグレーション（SQLファイル群）をデータベースに適用します。
///
/// 複数のプロセス（無停止更新で重なる新旧のサーバー・並列に走る結合テスト）が同時に呼んでも、
/// 適用するのは1つずつにする。sqlx の Migrator も advisory lock で直列化するが、それは
/// `pg_advisory_lock` で文の途中のまま待つ。その間に先行側が `CREATE INDEX CONCURRENTLY` を
/// 流すと、CONCURRENTLY は実行中のほかのトランザクション（＝ロック待ちの文）の終了を待つので
/// 互いに待ち合い、Postgres がデッドロックとして待っていた側を失敗させる。そこで先に
/// `pg_try_advisory_lock` を間隔を空けて取り直し（待つ側は短い文を繰り返すだけで、
/// CONCURRENTLY を止めない）、取れたプロセスだけが Migrator を動かす。
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    let mut conn = pool.acquire().await?;
    loop {
        let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1, $2)")
            .bind(MIGRATION_LOCK_KEY.0)
            .bind(MIGRATION_LOCK_KEY.1)
            .fetch_one(&mut *conn)
            .await?;
        if acquired {
            break;
        }
        tokio::time::sleep(MIGRATION_LOCK_RETRY_INTERVAL).await;
    }
    let result = sqlx::migrate!("./migrations").run(pool).await;
    // 外せなくても接続を閉じればロックは外れるので、失敗したら接続ごと捨てる。
    let unlocked = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1, $2)")
        .bind(MIGRATION_LOCK_KEY.0)
        .bind(MIGRATION_LOCK_KEY.1)
        .fetch_one(&mut *conn)
        .await;
    if !matches!(unlocked, Ok(true)) {
        tracing::warn!("[migrate] マイグレーションのロックを外せなかったので接続を閉じる");
        let _ = conn.detach().close().await;
    }
    result
}

/// SERIALIZABLE トランザクションの直列化失敗（40001）またはデッドロック検出（40P01）。
/// どちらもトランザクションを頭からやり直せば通りうる。
pub fn is_serialization_failure(err: &sqlx::Error) -> bool {
    matches!(
        err,
        sqlx::Error::Database(db_err)
            if matches!(db_err.code().as_deref(), Some("40001") | Some("40P01"))
    )
}

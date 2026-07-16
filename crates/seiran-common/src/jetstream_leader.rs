//! Jetstream 接続の排他制御（複数プロセス起動時のリーダー選出）。
//!
//! `docker-compose.mono.yml` の `--scale seiran-server=N`（無停止バージョンアップ中の
//! 一時的な複数起動）や、`firehose`ロールを複数インスタンス起動した場合、対策が無いと
//! Jetstream WebSocket接続がインスタンス数だけ重複して張られてしまう。Redisの
//! TTL付きリース（`SET NX EX`）でリーダーを1つに絞り、`firehose`/`all`ロールの制御
//! ループがこのリースの成否に応じてJetstream接続タスクを起動・停止する。
//!
//! プロセスIDではなくUUIDでリーダーを識別する（Dockerコンテナ間でPID 1が衝突するため）。
//! TTL更新はLuaスクリプトで「現在の値が自分のUUIDと一致する場合のみ延長」をアトミックに
//! 行う。GET→SETの2ステップに分けると、TTL失効の瞬間に他プロセスが横取りした直後に
//! 古いGET結果を根拠にSETしてしまい、奪い返す（split-brain）理論上の穴がある。

use std::time::Duration;

use redis::aio::ConnectionManager;
use uuid::Uuid;

/// リーダーを記録するRedisキー。
const LEADER_KEY: &str = "seiran:jetstream:leader";
/// リースのTTL（秒）。チェック間隔の2倍を確保し、通常運転では
/// リース確認のたびに残りTTLが半分以上残っている状態を保つ。
const LEASE_TTL_SECS: u64 = 10;
/// リーダー選出ループのチェック間隔。
pub const LEASE_CHECK_INTERVAL: Duration = Duration::from_secs(5);
/// Redis呼び出し（接続確立・リース確認）1回あたりのタイムアウト。`redis`クレートの
/// `ConnectionManager`はデフォルトで接続失敗時に内部リトライを行うため、Redis自体が
/// 応答しない状況では`ConnectionManager::new`が`LEASE_CHECK_INTERVAL`を超えて長時間
/// ブロックしうる（実測で確認済み）。ここでタイムアウトを切って必ず`Err`にし、
/// ポーリングループ（呼び出し元）が毎ティック確実にフェイルオープン/フェイルクローズの
/// 判定に戻れるようにする。
const REDIS_CALL_TIMEOUT: Duration = Duration::from_secs(3);

/// 自分の値と一致する場合のみTTLを延長するLuaスクリプト（アトミックなcompare-and-set）。
/// 戻り値は素直に整数（1=延長成功／0=既に他プロセスに奪われている）にする。
/// `redis.call('SET', ...)`の状態応答（"OK"）をそのまま返すと、Redisクライアント側の
/// bool変換規則に依存してしまい紛らわしいため避ける。
const RENEW_LUA: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then
    redis.call('SET', KEYS[1], ARGV[1], 'EX', ARGV[2])
    return 1
else
    return 0
end
"#;

pub struct JetstreamLeaderElector {
    conn: ConnectionManager,
    my_id: String,
    renew_script: redis::Script,
}

impl JetstreamLeaderElector {
    pub async fn connect(redis_url: &str) -> Result<Self, String> {
        tokio::time::timeout(REDIS_CALL_TIMEOUT, Self::connect_inner(redis_url))
            .await
            .map_err(|_| "Redis接続がタイムアウトしました".to_string())?
    }

    async fn connect_inner(redis_url: &str) -> Result<Self, String> {
        let client = redis::Client::open(redis_url)
            .map_err(|e| format!("Redis接続URLが不正です: {}", e))?;
        let conn = ConnectionManager::new(client)
            .await
            .map_err(|e| format!("Redis接続に失敗しました: {}", e))?;
        Ok(Self {
            conn,
            my_id: Uuid::new_v4().to_string(),
            renew_script: redis::Script::new(RENEW_LUA),
        })
    }

    /// リースの取得・延長を試みる。取れていれば`Ok(true)`（自分がリーダー）、
    /// 他プロセスが握っていれば`Ok(false)`。Redisとの通信自体に失敗（タイムアウト含む）
    /// した場合は`Err`を返す（呼び出し側でロールに応じたフェイルオープン/フェイルクローズを
    /// 判断する）。
    pub async fn try_acquire_or_renew(&self) -> Result<bool, String> {
        tokio::time::timeout(REDIS_CALL_TIMEOUT, self.try_acquire_or_renew_inner())
            .await
            .map_err(|_| "Redisリース確認がタイムアウトしました".to_string())?
    }

    async fn try_acquire_or_renew_inner(&self) -> Result<bool, String> {
        let mut conn = self.conn.clone();

        let acquired: Option<String> = redis::cmd("SET")
            .arg(LEADER_KEY)
            .arg(&self.my_id)
            .arg("NX")
            .arg("EX")
            .arg(LEASE_TTL_SECS)
            .query_async(&mut conn)
            .await
            .map_err(|e| e.to_string())?;

        if acquired.is_some() {
            return Ok(true);
        }

        let renewed: i64 = self
            .renew_script
            .key(LEADER_KEY)
            .arg(&self.my_id)
            .arg(LEASE_TTL_SECS)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| e.to_string())?;

        Ok(renewed == 1)
    }
}

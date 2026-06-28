use seiran_common::queue::{InMemoryJobQueue, WorkerEngine};
use seiran_common::traits::{Job, JobQueue};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    println!("[federation-worker] 起動中...");

    // オンメモリキューを生成
    let queue = Arc::new(InMemoryJobQueue::new());
    
    // WorkerEngine の初期化
    let engine = WorkerEngine::new(queue.clone());

    // ワーカーエンジンのバックグラウンド実行
    let _engine_handle = tokio::spawn(async move {
        engine.run().await;
    });

    println!("[federation-worker] テストジョブをエンキューします。");

    // 1. 優先度 NORMAL (10) でインバウンド処理
    queue.enqueue(
        Job::InboundActivityProcess {
            raw_activity: "{\"type\": \"Create\", \"object\": \"test\"}".to_string(),
        },
        10,
    ).await.unwrap();

    // 2. 優先度 CRITICAL (100) で ATP リポジトリ発行（最優先で実行される）
    queue.enqueue(
        Job::AtpRepositoryPublish {
            actor_id: 42,
            commit_type: "create_post".to_string(),
        },
        100,
    ).await.unwrap();

    // 3. 優先度 LOW (1) で過去ログ同期
    queue.enqueue(
        Job::ActorHistorySync {
            ap_uri: Some("https://example.com/users/mike".to_string()),
            at_did: None,
        },
        1,
    ).await.unwrap();

    // 4. 依存関係未解決による再スケジュールテスト用のジョブ
    queue.enqueue(
        Job::InboundActivityProcess {
            raw_activity: "{\"type\": \"Create\", \"object\": \"wait_dependency\"}".to_string(),
        },
        10,
    ).await.unwrap();

    // 動作ログが出揃うまで少し待つ
    tokio::time::sleep(tokio::time::Duration::from_secs(6)).await;

    println!("[federation-worker] 起動テスト終了。");
    Ok(())
}

//! ③ 配送受け入れ（インバウンド）キュー (`inbound_activity_process`)
//!
//! 外部（APのInboxやATPのFirehose）から届いたアクティビティを非同期解析し、DBに保存する。
//! 依存リソース（例: 返信先ポストや作成アクター）が未解決の場合、後で再処理するようにキューへ再スケジュールする。

use std::sync::Arc;
use crate::queue::worker::JobContext;
use crate::traits::Job;

pub async fn handle(raw_activity: String, ctx: Arc<JobContext>) -> Result<(), String> {
    println!(
        "[Job::InboundActivityProcess] 受信アクティビティパース中 (長さ: {})...",
        raw_activity.len()
    );

    // TODO: フェーズ4/5統合時に以下を実装
    // 1. JSON をパースして型判定 (Create, Follow, Undo, Like 等)
    // 2. 作成者アクターや、返信先ポストが自DBに存在するかチェック
    // 3. もし依存する親アクターや親ポストが未インポートなら、先にそちらを解決する必要がある
    
    // 仮の依存未解決シナリオのテスト・シミュレーション
    if raw_activity.contains("wait_dependency") {
        println!(
            "[Job::InboundActivityProcess] 警告: 依存関係 (親ポスト等) が未解決です。再スケジュールします。"
        );
        
        // 優先度 NORMAL (10) で再スケジュール
        ctx.queue
            .enqueue(
                Job::InboundActivityProcess {
                    raw_activity: raw_activity.replace("wait_dependency", "dependency_resolved"),
                },
                10,
            )
            .await
            .map_err(|e| format!("再スケジュール失敗: {}", e))?;
            
        return Ok(());
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;
    println!("[Job::InboundActivityProcess] 正常処理完了");
    Ok(())
}

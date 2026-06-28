//! ① 過去ログ同期キュー (`actor_history_sync`)
//!
//! 新規フォローされたアクターの過去ログ（最大300件）を取得・保存する。
//! ドメイン単位の同時実行制限（Concurrency Limit = 2）を適用する。

use std::sync::Arc;
use crate::queue::worker::JobContext;

pub async fn handle(
    ap_uri: Option<String>,
    at_did: Option<String>,
    ctx: Arc<JobContext>,
) -> Result<(), String> {
    let domain = if let Some(ref uri) = ap_uri {
        extract_domain(uri)
    } else if let Some(ref did) = at_did {
        // did:plc:xxxx や did:web:xxxx のプレフィックスを制限キーとして利用
        extract_did_prefix(did)
    } else {
        return Err("ap_uri または at_did のどちらかは必須です".to_string());
    };

    println!(
        "[Job::ActorHistorySync] ドメイン \"{}\" の並列セマフォを確保中...",
        domain
    );

    // ドメイン単位の同時実行制限（セマフォ）の取得
    let sem = ctx.get_domain_semaphore(&domain).await;
    let _permit = sem
        .acquire_owned()
        .await
        .map_err(|e| format!("セマフォ取得失敗: {}", e))?;

    println!(
        "[Job::ActorHistorySync] 開始 - ap_uri: {:?}, at_did: {:?}",
        ap_uri, at_did
    );

    // TODO: フェーズ4での通信エンジン統合時に以下を実装
    // 1. ActivityPub (Outbox) または ATP (getAuthorFeed) から過去ログを取得
    // 2. 過去30日間 / 最大300件の制約を適用してローカルDBに保存
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    println!("[Job::ActorHistorySync] 正常終了");
    Ok(())
}

fn extract_domain(uri: &str) -> String {
    // 簡易的なドメイン抽出ロジック (e.g. "https://example.com/users/test" -> "example.com")
    if let Some(stripped) = uri.strip_prefix("https://") {
        stripped.split('/').next().unwrap_or("unknown_ap").to_string()
    } else if let Some(stripped) = uri.strip_prefix("http://") {
        stripped.split('/').next().unwrap_or("unknown_ap").to_string()
    } else {
        uri.to_string()
    }
}

fn extract_did_prefix(did: &str) -> String {
    // did:plc:123 -> did:plc
    let parts: Vec<&str> = did.split(':').collect();
    if parts.len() >= 2 {
        format!("{}:{}", parts[0], parts[1])
    } else {
        did.to_string()
    }
}

//! ② 投稿配送キュー (`outbound_post_delivery`)
//!
//! 自住民の新規投稿を外部連合サーバー（ActivityPubフォロワーのInbox等）へ配送する。
//! 高優先度で処理される。

use std::sync::Arc;
use crate::queue::worker::JobContext;

pub async fn handle(post_id: i64, _ctx: Arc<JobContext>) -> Result<(), String> {
    println!("[Job::OutboundPostDelivery] 開始 - post_id: {}", post_id);

    // TODO: フェーズ4での通信エンジン統合時に以下を実装
    // 1. DBから投稿情報をロード (sqlx)
    // 2. 配信対象のリモートアクター (ActivityPub フォロワー一覧など) を抽出
    // 3. 各相手サーバーの Inbox に対し HTTP Signatures で署名した HTTP POST を実行
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    // 例として仮に配信成功とする
    println!("[Job::OutboundPostDelivery] 投稿配送成功: post_id={}", post_id);
    Ok(())
}

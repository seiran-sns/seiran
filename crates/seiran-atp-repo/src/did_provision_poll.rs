//! 「DNS TXT 登録完了待ち」アカウントの DID 発行を進める定時処理
//! （`seiran_common::did_provision`）。

use std::sync::Arc;
use std::time::Duration;

use seiran_common::CancellationToken;
use seiran_common::did_provision::DidProvisioner;

/// TXT の公開確認を問い合わせる間隔。登録から DID 確定までの待ち時間にそのまま効くので短くする。
/// 待ちの行が無ければ1文の SELECT だけで終わる。
const POLL_INTERVAL: Duration = Duration::from_secs(2);

pub async fn run(provisioner: Arc<DidProvisioner>, stop: CancellationToken) {
    seiran_common::leader::every(POLL_INTERVAL, stop, || provisioner.run_pending()).await;
}

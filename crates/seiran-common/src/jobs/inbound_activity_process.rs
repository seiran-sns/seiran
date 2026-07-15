//! ③ 配送受け入れ（インバウンド）キュー (`inbound_activity_process`)
//!
//! 外部（AP の Inbox 等）から届いたアクティビティのうち、Inbox ハンドラが
//! 個別処理していない種別を非同期解析・保存するためのジョブ。
//!
//! **未実装**: 現状、既知のアクティビティ（Follow/Create/Accept/Undo/Announce/Like）は
//! `seiran-federation-inbox` の各ハンドラが処理しており、ここへ来るのは未対応種別のみ。
//! Inbox ハンドラ群の Worker への移設（改善レポート A-3 / 改修計画 #9）の際に、
//! ここが処理本体になる。

use std::sync::Arc;
use crate::queue::worker::JobContext;

pub async fn handle(raw_activity: String, _ctx: Arc<JobContext>) -> Result<(), String> {
    let activity_type = serde_json::from_str::<serde_json::Value>(&raw_activity)
        .ok()
        .and_then(|v| v["type"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "(不明)".to_string());

    eprintln!(
        "[Job::InboundActivityProcess] type={} は未実装のため破棄します ({} bytes)",
        activity_type,
        raw_activity.len()
    );
    Ok(())
}

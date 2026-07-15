//! ④ アクター検証・メタデータ取得キュー (`actor_metadata_resolve`)
//!
//! リモートアクターの Webfinger 解決、Bio に埋め込まれた /verify-actor ハンドシェイク署名の
//! 検証、およびプロフィール画像・アバター等のキャッシュ更新処理を行う（予定）。
//!
//! **未実装**: ゼロトラストペアリング統合（フェーズ4）で実装する。
//! 現状 enqueue している箇所はない。

use std::sync::Arc;
use crate::queue::worker::JobContext;

pub async fn handle(actor_id: i64, _ctx: Arc<JobContext>) -> Result<(), String> {
    eprintln!(
        "[Job::ActorMetadataResolve] 未実装のため破棄します (actor_id={})",
        actor_id
    );
    Ok(())
}

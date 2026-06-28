//! ④ アクター検証・メタデータ取得キュー (`actor_metadata_resolve`)
//!
//! リモートアクターのWebfinger解決、Bioに埋め込まれた /verify-actor ハンドシェイク署名の検証、
//! およびプロフィール画像・アバター等のキャッシュ更新処理を行う。

use std::sync::Arc;
use crate::queue::worker::JobContext;

pub async fn handle(actor_id: i64, _ctx: Arc<JobContext>) -> Result<(), String> {
    println!("[Job::ActorMetadataResolve] 開始 - actor_id: {}", actor_id);

    // TODO: フェーズ4のゼロトラストペアリング統合時に以下を実装
    // 1. DBから対照アクターの情報をロード
    // 2. Webfinger または DID ドキュメントを解決
    // 3. `/verify-actor` 特権エンドポイントでチャレンジ＆レスポンス検証
    // 4. アバターURL等のキャッシュ更新
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    println!(
        "[Job::ActorMetadataResolve] メタデータ解決完了: actor_id={}",
        actor_id
    );
    Ok(())
}

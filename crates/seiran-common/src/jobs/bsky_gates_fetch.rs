//! Bsky投稿の返信許可（threadgate）・引用可否（postgate）を後から取得して保存する
//! （`Job::BskyGatesFetch`）。検索結果の取り込みで、保存の応答を待たせないために使う。

use std::sync::Arc;

use crate::queue::worker::JobContext;

pub async fn handle(
    post_id: i64,
    author_did: String,
    at_uri: String,
    ctx: Arc<JobContext>,
) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!(
            "[BskyGatesFetch] DB pool未設定のためスキップ (post_id={})",
            post_id
        );
        return Ok(());
    };
    let http = ctx.ap_client.http.as_ref();
    // 取得失敗時は両方とも「制限なし」（誤ってボタンをグレーアウトしない）。
    let (reply_allow, quote_disabled) =
        crate::atp::fetch_bsky_gates(http, &author_did, &at_uri).await;
    crate::repository::bsky_ingest::set_bsky_gates(
        pool,
        post_id,
        reply_allow.as_ref(),
        quote_disabled,
    )
    .await
    .map_err(|e| format!("gate情報の保存失敗 post_id={post_id}: {e}"))
}

//! リモート（seiranユーザー所有でない）Bskyリストの全メンバーDIDを取得し、
//! `bsky_remote_list_membership_cache`へ24時間TTLで保存する。threadgateのlistRule評価
//! （`docs/protocols.md`参照）でキャッシュ未登録/期限切れのリストを見つけた際に積まれる。

use std::sync::Arc;

use crate::atp::fetch_bsky_list_members;
use crate::queue::worker::JobContext;

pub async fn handle(list_uri: String, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!("[BskyListMembershipResolve] DB pool未設定のためスキップ");
        return Ok(());
    };

    let members = fetch_bsky_list_members(&ctx.ap_client.http, &list_uri).await;
    let member_dids = serde_json::json!(members);

    sqlx::query(
        "INSERT INTO bsky_remote_list_membership_cache (list_uri, member_dids, checked_at)
         VALUES ($1, $2, now())
         ON CONFLICT (list_uri) DO UPDATE SET member_dids = $2, checked_at = now()",
    )
    .bind(&list_uri)
    .bind(&member_dids)
    .execute(pool)
    .await
    .map_err(|e| format!("bsky_remote_list_membership_cache 保存失敗: {}", e))?;

    tracing::info!(
        "[BskyListMembershipResolve] キャッシュ更新完了 list={} members={}",
        list_uri,
        members.len()
    );
    Ok(())
}

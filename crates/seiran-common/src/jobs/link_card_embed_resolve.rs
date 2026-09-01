//! Bsky受信投稿のURLカードに対し、oEmbed discoveryで見つかった埋め込みプレーヤーの
//! iframe srcを後追いでUPDATEする。Bskyの`app.bsky.embed.external`にはiframe情報が無く、
//! title/description/thumbnailは既に同期的にINSERT済みのため、このジョブはUPDATEのみ行う。
//! `post_link_cards(post_id, position)`にUNIQUE制約があるため、対象行は常に高々1件に
//! 一意に定まる。

use std::sync::Arc;

use crate::net::{fetch_ogp, FetchError};
use crate::queue::worker::JobContext;

pub async fn handle(
    post_id: i64,
    position: i16,
    url: String,
    ctx: Arc<JobContext>,
) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!(
            "[LinkCardEmbedResolve] DB pool未設定のためスキップ (post_id={})",
            post_id
        );
        return Ok(());
    };
    let Some(whitelist) = &ctx.oembed_whitelist else {
        tracing::warn!(
            "[LinkCardEmbedResolve] whitelist未設定のためスキップ (post_id={})",
            post_id
        );
        return Ok(());
    };

    let fixed_endpoint = whitelist.fixed_endpoint_for(&url).await;
    let ogp = match fetch_ogp(&url, fixed_endpoint.as_deref()).await {
        Ok(Some(ogp)) => ogp,
        Ok(None) => return Ok(()), // og:titleもoEmbedも無し。GenericCardのまま諦める。
        Err(FetchError::FetchFailed | FetchError::UpstreamError | FetchError::DnsFailed) => {
            return Err(format!("embed解決失敗（リトライ対象）: url={url}"));
        }
        Err(e) => {
            tracing::info!(
                "[LinkCardEmbedResolve] 取得を諦めます url={} reason={}",
                url,
                e
            );
            return Ok(());
        }
    };

    let Some(embed_src) = whitelist.filter_embed_src(ogp.embed_src.as_deref()).await else {
        return Ok(()); // oEmbed非対応サイト、またはホワイトリスト外。GenericCardのまま。
    };

    sqlx::query(
        "UPDATE post_link_cards SET embed_src = $1, embed_type = $2 WHERE post_id = $3 AND position = $4",
    )
    .bind(&embed_src)
    .bind(&ogp.embed_type)
    .bind(post_id)
    .bind(position)
    .execute(pool)
    .await
    .map_err(|e| format!("post_link_cards UPDATE失敗: {}", e))?;

    tracing::info!(
        "[LinkCardEmbedResolve] embed_src保存完了 post_id={} url={}",
        post_id,
        url
    );
    Ok(())
}

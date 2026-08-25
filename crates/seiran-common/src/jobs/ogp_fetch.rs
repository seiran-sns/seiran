//! Fedi受信投稿の本文中URLのOGPを取得し`post_link_cards`へ保存する。取得できなければ
//! 静かに諦める（投稿自体は既に保存済みのため、そのURLがカード無しのまま表示されるだけで
//! 実害は小さい）。oEmbed discoveryで見つかった埋め込みプレーヤー（`embed_src`/`embed_type`）
//! も、ホワイトリスト（`oembed_whitelist`）判定を通した上で同じ行に保存する。
//!
//! OGP取得・抽出そのものは `crate::net::fetch_ogp`（SSRF対策込み）を使う。ローカル作成投稿の
//! Bsky embed選択（#227、URLカード選択時）も同じ関数を同期的に呼び出して共有している。

use std::sync::Arc;

use crate::net::{fetch_ogp, FetchError};
use crate::queue::worker::JobContext;

pub async fn handle(
    post_id: i64,
    url: String,
    position: i16,
    ctx: Arc<JobContext>,
) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!(
            "[OgpFetch] DB pool 未設定のためスキップ (post_id={})",
            post_id
        );
        return Ok(());
    };

    let fixed_endpoint = match &ctx.oembed_whitelist {
        Some(whitelist) => whitelist.fixed_endpoint_for(&url).await,
        None => None,
    };
    let ogp = match fetch_ogp(&url, fixed_endpoint.as_deref()).await {
        Ok(Some(ogp)) => ogp,
        Ok(None) => {
            tracing::info!(
                "[OgpFetch] og:title・oEmbedいずれも見つからないため諦めます url={}",
                url
            );
            return Ok(());
        }
        Err(FetchError::FetchFailed | FetchError::UpstreamError | FetchError::DnsFailed) => {
            // 一時的な失敗の可能性があるためリトライさせる。
            return Err(format!("OGP取得失敗（リトライ対象）: url={url}"));
        }
        Err(e) => {
            // 不正なURL・プライベートアドレス・非対応Content-Type等は再試行しても無駄なので諦める。
            tracing::info!("[OgpFetch] 取得を諦めます url={} reason={}", url, e);
            return Ok(());
        }
    };

    let embed_src = match &ctx.oembed_whitelist {
        Some(whitelist) => whitelist.filter_embed_src(ogp.embed_src.as_deref()).await,
        None => None,
    };
    let embed_type = embed_src.as_ref().and_then(|_| ogp.embed_type.clone());

    sqlx::query(
        "INSERT INTO post_link_cards (post_id, position, url, title, description, thumbnail_url, embed_src, embed_type)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(post_id)
    .bind(position)
    .bind(&url)
    .bind(&ogp.title)
    .bind(&ogp.description)
    .bind(&ogp.thumbnail_url)
    .bind(&embed_src)
    .bind(&embed_type)
    .execute(pool)
    .await
    .map_err(|e| format!("post_link_cards INSERT失敗: {}", e))?;

    tracing::info!("[OgpFetch] 保存完了 post_id={} url={}", post_id, url);
    Ok(())
}

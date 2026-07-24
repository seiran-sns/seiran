//! 未知のリモート Fedi アクター（ローカル `actors` 未登録）のプロフィールを解決してキャッシュ
//! するジョブ (`RemoteActorResolve`, #68 マイケル指摘)。
//!
//! リモートの followers/following 一覧取得（同期取得・`RemoteFollowListSync` バックグラウンド
//! 取得の双方）で、ローカル DB に存在しない actor URI が見つかった場合に積まれる。
//! フォロー関係は作らず、`actors` テーブルへの upsert のみ行う（表示のリッチ化が目的で、
//! この時点でフォロー関係は発生していないため）。

use std::sync::Arc;

use crate::generate_snowflake_id;
use crate::queue::worker::JobContext;
use crate::repository::{ActorRepository, PgActorRepository};

fn extract_domain(uri: &str) -> String {
    if let Some(s) = uri.strip_prefix("https://") {
        s.split('/').next().unwrap_or("unknown").to_string()
    } else if let Some(s) = uri.strip_prefix("http://") {
        s.split('/').next().unwrap_or("unknown").to_string()
    } else {
        uri.to_string()
    }
}

pub async fn handle(uri: String, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!("[RemoteActorResolve] DB pool 未設定のためスキップ (uri={})", uri);
        return Ok(());
    };

    let actor_repo = PgActorRepository::new(pool.clone());
    if actor_repo
        .find_by_ap_uri(&uri)
        .await
        .map_err(|e| format!("DB検索失敗: {}", e))?
        .is_some()
    {
        // 既に他経路（フォロー等）で解決済み。
        return Ok(());
    }

    let domain = extract_domain(&uri);
    let sem = ctx.get_domain_semaphore(&domain).await;
    let _permit = sem.acquire_owned().await.map_err(|e| format!("セマフォ取得失敗: {}", e))?;

    let actor = ctx
        .ap_client
        .fetch_actor(&uri)
        .await
        .map_err(|e| format!("アクタードキュメント取得失敗: {}", e))?;

    let Some(inbox) = actor.inbox.clone() else {
        tracing::info!("[RemoteActorResolve] inbox が無いためスキップ: {}", uri);
        return Ok(());
    };

    let avatar_url = actor.avatar_url();
    let username = actor
        .preferred_username
        .clone()
        .unwrap_or_else(|| uri.rsplit('/').next().unwrap_or("unknown").to_string());
    let display_name = actor.name.clone().unwrap_or_else(|| username.clone());
    let bio = actor.summary.as_deref().map(crate::jobs::inbound_activity_process::strip_html);
    let emoji_map = actor.emoji_map();
    let profile_fields = actor.profile_fields_json();

    let new_id = generate_snowflake_id(chrono::Utc::now());
    actor_repo
        .upsert_remote_fedi(
            new_id,
            &uri,
            &inbox,
            &username,
            &domain,
            &display_name,
            avatar_url.as_deref(),
            bio.as_deref(),
            chrono::Utc::now(),
            &emoji_map,
            &profile_fields,
        )
        .await
        .map_err(|e| format!("upsert_remote_fedi 失敗: {}", e))?;

    tracing::info!("[RemoteActorResolve] 未知アクター解決完了: uri={} handle={}", uri, username);
    Ok(())
}

//! bio/profile_fields中の未解決URLを、Fedi（ActivityPubのContent-Negotiation直接フェッチ、
//! WebFingerは`acct:`形式の解決にしか使えないため使わない）またはBsky（AppView経由）で
//! 判定し、`link_resolutions`テーブルへ結果（陽性/陰性）を保存するジョブ（`Job::LinkResolve`）。
//!
//! プロフィール取得API（`seiran-api`）が、bio/profile_fields中の未キャッシュURL・TTL経過済み
//! 陰性URLに対してこのジョブを積む。陽性が得られればストリーミングで`linkResolved`を
//! ログイン中クライアントへ通知する（未ログイン閲覧者への配送は行わない）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::link_target::{self, ResolveContext, ResolveError, ResolvedTarget};
use crate::queue::worker::JobContext;
use crate::repository::{ActorRepository, PgActorRepository, PgPostRepository};

/// `remote_actor_resolve`と同様、URL単位の重複投入防止クールダウン（プロセス内グローバル）。
const LINK_RESOLVE_COOLDOWN: Duration = Duration::from_secs(600);

fn recent_map() -> &'static Mutex<HashMap<String, Instant>> {
    static MAP: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `Job::LinkResolve`をenqueueしてよいか判定する。trueを返した場合のみ実際にenqueueすること。
pub fn should_enqueue(url: &str) -> bool {
    let now = Instant::now();
    let mut map = recent_map().lock().expect("recent_map mutex poisoned");
    if let Some(last) = map.get(url) {
        if now.duration_since(*last) < LINK_RESOLVE_COOLDOWN {
            return false;
        }
    }
    map.insert(url.to_string(), now);
    true
}

pub async fn handle(url: String, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!("[LinkResolve] DB pool 未設定のためスキップ (url={})", url);
        return Ok(());
    };
    let local_domain = ctx
        .inbox
        .as_ref()
        .map(|i| i.local_domain.as_str())
        .or_else(|| ctx.delivery.as_ref().map(|d| d.local_domain.as_str()));
    let Some(local_domain) = local_domain else {
        tracing::warn!(
            "[LinkResolve] local_domain 未設定のためスキップ (url={})",
            url
        );
        return Ok(());
    };

    let actors = PgActorRepository::new(pool.clone());
    let posts = PgPostRepository::new(pool.clone());
    let resolve_ctx = ResolveContext {
        actors: &actors,
        posts: &posts,
        ap_client: &ctx.ap_client,
        job_queue: &ctx.queue,
        db_pool: pool,
        local_domain,
        system_signing_key: ctx.system_signing_key(),
    };

    let (kind, resolved_actor_id, resolved_post_id) = match link_target::parse_target(&url) {
        None => ("none", None, None),
        Some(parsed) => match link_target::resolve_target(&resolve_ctx, parsed).await {
            Ok(ResolvedTarget::Actor(actor)) => ("actor", Some(actor.id), None),
            Ok(ResolvedTarget::Post(post_id)) => ("post", None, Some(post_id)),
            Err(ResolveError::ImportPending) => {
                // 取り込み自体は継続中。今回は陰性扱いにせず、次回の再アクセス時に
                // 改めてenqueueされるのに任せる（クールダウンだけ更新済み）。
                tracing::info!("[LinkResolve] 投稿取り込み待機中: {}", url);
                return Ok(());
            }
            Err(_) => ("none", None, None),
        },
    };

    sqlx::query(
        "INSERT INTO link_resolutions (url, kind, resolved_actor_id, resolved_post_id, checked_at)
         VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (url) DO UPDATE SET
             kind = EXCLUDED.kind,
             resolved_actor_id = EXCLUDED.resolved_actor_id,
             resolved_post_id = EXCLUDED.resolved_post_id,
             checked_at = EXCLUDED.checked_at",
    )
    .bind(&url)
    .bind(kind)
    .bind(resolved_actor_id)
    .bind(resolved_post_id)
    .execute(pool)
    .await
    .map_err(|e| format!("link_resolutions UPSERT失敗: {}", e))?;

    if kind == "none" {
        return Ok(());
    }

    let stream_hub = ctx
        .inbox
        .as_ref()
        .map(|i| i.stream_hub.clone())
        .or_else(|| ctx.follow_exec.as_ref().map(|f| f.stream_hub.clone()));
    let Some(stream_hub) = stream_hub else {
        return Ok(());
    };

    let mut body = serde_json::json!({ "url": url, "kind": kind });
    if let Some(actor_id) = resolved_actor_id {
        if let Ok(Some(actor)) = actors.find_by_id(actor_id).await {
            let avatar_url = actors.find_avatar_url(actor_id).await.unwrap_or(None);
            let avatar_url = crate::avatar::resolve_avatar_url(
                avatar_url,
                &actor.actor_type,
                &actor.domain,
                actor_id,
            );
            body["username"] = serde_json::json!(actor.username);
            body["domain"] = serde_json::json!(actor.domain);
            body["actorType"] = serde_json::json!(actor.actor_type);
            body["actorId"] = serde_json::json!(actor_id.to_string());
            body["avatarUrl"] = serde_json::json!(avatar_url);
        }
    }
    if let Some(post_id) = resolved_post_id {
        body["postId"] = serde_json::json!(post_id.to_string());
    }
    stream_hub.publish_broadcast("linkResolved", body);

    Ok(())
}

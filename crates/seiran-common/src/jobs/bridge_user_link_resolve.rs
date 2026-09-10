//! `Job::BridgeUserLinkResolve`（`docs/protocols.md`参照）: brid.gy(Bridgy Fed)ブリッジ
//! ユーザーの実ユーザーへのリンク（`actors.bridge_real_actor_id`）を解決する。プロフィール
//! 表示のたびに、未解決なブリッジユーザーに対して積まれる（「表示時再検証」パターン、
//! `also_known_as_verify`等と同様）。ただしブリッジ関係自体は不変のため、一度解決すれば
//! 以後は再検証しない（`also_known_as_verify`とはこの点で異なる）。
//!
//! - AP側ブリッジユーザー（`bsky.brid.gy`ドメイン、Bridgy FedがBluesky上の実ユーザーを
//!   AP側へ投影したもの）: `ap_uri`が`https://bsky.brid.gy/ap/{did}`の形で実ユーザーの
//!   DIDをそのまま持つため、ネットワーク取得不要で抽出できる（実データで確認済み）。
//! - ATP側ブリッジユーザー（`*.ap.brid.gy`ハンドル、Bridgy FedがFediverse上の実ユーザーを
//!   ATP側へ投影したもの）: ハンドル（`{username}.{domain}.ap.brid.gy`）から実ユーザーの
//!   username/domainを復元し、webfingerで実ユーザーのAP actor URIを解決する
//!   （`mention.rs`が逆方向にこのハンドルを組み立てる際と同じ規約）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::generate_snowflake_id;
use crate::queue::worker::JobContext;
use crate::repository::{Actor, ActorRepository, PgActorRepository};

/// enqueue元（プロフィール表示のたび）の重複投入防止クールダウン。ブリッジ関係の解決
/// そのものは一度成功すれば`handle`冒頭のガードで以後スキップされるため、これは
/// 「解決に失敗し続けている」ケースで同じブリッジユーザーが短時間に何度も表示された際の
/// 過剰なwebfinger/フェッチ呼び出しを防ぐためのもの（`remote_actor_resolve`と同じ設計）。
const BRIDGE_USER_LINK_RESOLVE_COOLDOWN: Duration = Duration::from_secs(3600);

fn recent_map() -> &'static Mutex<HashMap<i64, Instant>> {
    static MAP: OnceLock<Mutex<HashMap<i64, Instant>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `Job::BridgeUserLinkResolve`をenqueueしてよいか判定する。
pub fn should_enqueue(actor_id: i64) -> bool {
    let now = Instant::now();
    let mut map = recent_map().lock().expect("recent_map mutex poisoned");
    if let Some(last) = map.get(&actor_id) {
        if now.duration_since(*last) < BRIDGE_USER_LINK_RESOLVE_COOLDOWN {
            return false;
        }
    }
    map.insert(actor_id, now);
    true
}

pub async fn handle(actor_id: i64, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!(
            "[BridgeUserLinkResolve] DB pool 未設定のためスキップ (actor_id={})",
            actor_id
        );
        return Ok(());
    };
    let actor_repo = PgActorRepository::new(pool.clone());
    let Some(actor) = actor_repo
        .find_by_id(actor_id)
        .await
        .map_err(|e| format!("アクター取得失敗: {}", e))?
    else {
        return Ok(());
    };
    // 既に解決済みならブリッジ関係自体は不変のため何もしない。
    if actor.bridge_real_actor_id.is_some() {
        return Ok(());
    }

    let real_id = if actor.domain == "bsky.brid.gy" {
        resolve_ap_side_bridge(&actor_repo, &ctx, &actor).await?
    } else if actor.username.ends_with(".ap.brid.gy") {
        resolve_atp_side_bridge(&actor_repo, &ctx, &actor).await?
    } else {
        None
    };

    if let Some(real_id) = real_id {
        sqlx::query("UPDATE actors SET bridge_real_actor_id = $1 WHERE id = $2")
            .bind(real_id)
            .bind(actor_id)
            .execute(pool)
            .await
            .map_err(|e| format!("bridge_real_actor_id 更新失敗: {}", e))?;
        tracing::info!(
            "[BridgeUserLinkResolve] 解決完了: actor_id={} real_actor_id={}",
            actor_id,
            real_id
        );
    }
    Ok(())
}

/// AP側ブリッジユーザー（`bsky.brid.gy`）: `ap_uri`から実ユーザーのDIDを直接取り出す。
/// 既知ならそのまま返し、未知ならAppViewからプロフィールを取得して`upsert_remote_bsky`する。
async fn resolve_ap_side_bridge(
    actor_repo: &PgActorRepository,
    ctx: &Arc<JobContext>,
    actor: &Actor,
) -> Result<Option<i64>, String> {
    let Some(did) = actor
        .ap_uri
        .as_deref()
        .and_then(|uri| uri.strip_prefix("https://bsky.brid.gy/ap/"))
    else {
        return Ok(None);
    };

    if let Some(real) = actor_repo
        .find_by_did(did)
        .await
        .map_err(|e| format!("DB検索失敗: {}", e))?
    {
        return Ok(Some(real.id));
    }

    let http = ctx.ap_client.http.as_ref();
    let profile = match crate::atp::fetch_bsky_profile(http, did).await {
        Ok(p) => p,
        Err(e) => {
            tracing::info!(
                "[BridgeUserLinkResolve] 実ユーザープロフィール取得失敗（諦めます） did={}: {}",
                did,
                e
            );
            return Ok(None);
        }
    };
    let new_id = generate_snowflake_id(chrono::Utc::now());
    let real_id = actor_repo
        .upsert_remote_bsky(
            new_id,
            did,
            &profile.handle,
            profile.display_name.as_deref(),
            profile.avatar.as_deref(),
            chrono::Utc::now(),
        )
        .await
        .map_err(|e| format!("upsert_remote_bsky 失敗: {}", e))?;
    Ok(Some(real_id))
}

/// ATP側ブリッジユーザー（`*.ap.brid.gy`）: ハンドルから実ユーザーのusername/domainを
/// 復元し、webfingerで実ユーザーのAP actor URIを解決する。既知ならそのまま返し、未知なら
/// `remote_actor_resolve`（#68の既存ジョブ、`RemoteActorResolve`）の取得・upsertパイプラインを
/// 再利用して実体化する。
async fn resolve_atp_side_bridge(
    actor_repo: &PgActorRepository,
    ctx: &Arc<JobContext>,
    actor: &Actor,
) -> Result<Option<i64>, String> {
    let Some(without_suffix) = actor.username.strip_suffix(".ap.brid.gy") else {
        return Ok(None);
    };
    // ハンドル規約は`{username}.{domain}.ap.brid.gy`（`mention.rs`が逆方向に組み立てる形と
    // 同じ）。AP usernameは通常ドットを含まないため、最初のドットで区切ればusername/domainに
    // 一意に分解できる。
    let Some(dot) = without_suffix.find('.') else {
        return Ok(None);
    };
    let (username, domain) = (&without_suffix[..dot], &without_suffix[dot + 1..]);

    if let Some(real) = actor_repo
        .find_by_username_domain(username, domain)
        .await
        .map_err(|e| format!("DB検索失敗: {}", e))?
    {
        return Ok(Some(real.id));
    }

    let ap_uri = match ctx.ap_client.resolve_webfinger(username, domain).await {
        Ok(uri) => uri,
        Err(e) => {
            tracing::info!(
                "[BridgeUserLinkResolve] webfinger解決失敗（諦めます） {}@{}: {}",
                username,
                domain,
                e
            );
            return Ok(None);
        }
    };

    // 未知の実ユーザー: `RemoteActorResolve`と同じ取得・upsertパイプラインを再利用する
    // （フェッチ失敗時はログのみでこちらも諦める。次回のプロフィール表示で再試行される）。
    if let Err(e) = crate::jobs::remote_actor_resolve::handle(ap_uri.clone(), ctx.clone()).await {
        tracing::info!(
            "[BridgeUserLinkResolve] 実ユーザー解決失敗（諦めます） ap_uri={}: {}",
            ap_uri,
            e
        );
        return Ok(None);
    }
    let real = actor_repo
        .find_by_ap_uri(&ap_uri)
        .await
        .map_err(|e| format!("DB検索失敗: {}", e))?;
    Ok(real.map(|r| r.id))
}

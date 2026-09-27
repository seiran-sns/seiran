//! リモート Fedi アクターの followers/following 全件同期キュー (`remote_follow_list_sync`, #68)
//!
//! プロフィール表示時の短タイムアウト同期取得が失敗/タイムアウトした場合に積まれる。
//! `fetch_ap_collection_uris` で OrderedCollection をページ辿りしながら全件取得し、
//! `remote_follow_snapshots` へ丸ごと upsert する。ドメイン単位の同時実行制限
//! （`ActorHistorySync` と同じ Concurrency Limit = 2）を適用する。

use std::sync::Arc;

use crate::repository::ActorRepository;

use crate::ap::fetch_ap_collection_uris;
use crate::queue::worker::JobContext;

/// 1回のジョブで取得する actor URI の上限。プロフィール表示時の同期取得（数百件程度の
/// キャップ）より大幅に緩め、バックグラウンドで現実的な規模のアカウントを網羅する。
const MAX_ITEMS: usize = 5000;

pub async fn handle(actor_id: i64, direction: String, ctx: Arc<JobContext>) -> Result<(), String> {
    if direction != "following" && direction != "followers" {
        return Err(format!("不正な direction です: {}", direction));
    }

    let pool = ctx
        .db_pool
        .as_ref()
        .ok_or_else(|| "DB pool 未設定".to_string())?;

    let actor = crate::repository::PgActorRepository::new(pool.clone())
        .find_by_id(actor_id)
        .await
        .map_err(|e| format!("アクターDB検索失敗: {}", e))?;
    let ap_uri: String = match actor.and_then(|a| a.ap_uri) {
        Some(uri) => uri,
        None => {
            tracing::warn!(
                "[RemoteFollowListSync] actor_id={} は ap_uri を持たない（ローカル/Bskyアクター、スキップ）",
                actor_id
            );
            return Ok(());
        }
    };

    let domain = extract_domain(&ap_uri);
    let sem = ctx.get_domain_semaphore(&domain).await;
    let _permit = sem
        .acquire_owned()
        .await
        .map_err(|e| format!("セマフォ取得失敗: {}", e))?;

    tracing::info!(
        "[RemoteFollowListSync] 開始: actor_id={} direction={} ({})",
        actor_id,
        direction,
        ap_uri
    );

    // Authorized Fetch（secure mode）対応。
    let signing_key = ctx.system_signing_key();

    let actor = ctx
        .ap_client
        .fetch_actor_with_key(&ap_uri, crate::ap::client::signing_key_refs(&signing_key))
        .await
        .map_err(|e| format!("アクタードキュメント取得失敗: {}", e))?;

    let collection_url = match direction.as_str() {
        "following" => actor.following,
        _ => actor.followers,
    };
    let collection_url = match collection_url {
        Some(url) => url,
        None => {
            tracing::info!(
                "[RemoteFollowListSync] {} フィールドが存在しません（非対応実装、スキップ）: {}",
                direction,
                ap_uri
            );
            return Ok(());
        }
    };

    let (uris, complete) = fetch_ap_collection_uris(
        &ctx.ap_client,
        &collection_url,
        MAX_ITEMS,
        signing_key.as_ref().map(|(k, p)| (k.as_str(), p.as_str())),
    )
    .await;
    tracing::info!(
        "[RemoteFollowListSync] {}件取得完了 (complete={}): actor_id={} direction={}",
        uris.len(),
        complete,
        actor_id,
        direction
    );

    crate::repository::remote_follow_snapshot::save(pool, actor_id, &direction, &uris, complete)
        .await
        .map_err(|e| format!("スナップショット保存失敗: {}", e))?;

    enqueue_unknown_actor_resolves(pool, &ctx.queue, &uris).await;

    tracing::info!(
        "[RemoteFollowListSync] 完了: actor_id={} direction={}",
        actor_id,
        direction
    );
    Ok(())
}

/// 取得した actor URI のうち、ローカル `actors` に未登録のものについて `RemoteActorResolve`
/// ジョブを積む（#68）。
async fn enqueue_unknown_actor_resolves(
    pool: &sqlx::PgPool,
    queue: &Arc<dyn crate::traits::JobQueue>,
    uris: &[String],
) {
    let known: std::collections::HashSet<String> =
        crate::repository::remote_follow_snapshot::known_actors_by_ap_uris(pool, uris)
            .await
            .map(|rows| rows.into_iter().map(|r| r.ap_uri).collect())
            .unwrap_or_default();

    for uri in uris {
        if known.contains(uri) {
            continue;
        }
        // クールダウン中（直近解決を試みたが未解決のまま等）ならスキップする。
        // フォロー数の多いアクター1件でも数百〜数千URIの束になるため、ここに歯止めが
        // 無いと#68の趣旨（表示のリッチ化）に見合わない負荷になる。
        if !crate::jobs::remote_actor_resolve::should_enqueue(uri) {
            continue;
        }
        if let Err(e) = queue
            .enqueue(
                crate::traits::Job::RemoteActorResolve { uri: uri.clone() },
                crate::queue::worker::priority::LOW,
            )
            .await
        {
            tracing::warn!(
                "[RemoteFollowListSync] RemoteActorResolve enqueue失敗 (uri={}): {}",
                uri,
                e
            );
        }
    }
}

fn extract_domain(uri: &str) -> String {
    if let Some(s) = uri.strip_prefix("https://") {
        s.split('/').next().unwrap_or("unknown").to_string()
    } else if let Some(s) = uri.strip_prefix("http://") {
        s.split('/').next().unwrap_or("unknown").to_string()
    } else {
        uri.to_string()
    }
}

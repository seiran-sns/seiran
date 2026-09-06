//! 未知のリモート Fedi アクター（ローカル `actors` 未登録）のプロフィールを解決してキャッシュ
//! するジョブ (`RemoteActorResolve`, #68 マイケル指摘)。
//!
//! リモートの followers/following 一覧取得（同期取得・`RemoteFollowListSync` バックグラウンド
//! 取得の双方）で、ローカル DB に存在しない actor URI が見つかった場合に積まれる。
//! フォロー関係は作らず、`actors` テーブルへの upsert のみ行う（表示のリッチ化が目的で、
//! この時点でフォロー関係は発生していないため）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::generate_snowflake_id;
use crate::queue::worker::JobContext;
use crate::repository::{ActorRepository, PgActorRepository};

/// enqueue元（APIハンドラの`remote_follow_summary`・Worker側の`RemoteFollowListSync`の
/// 双方）で共有する重複投入防止クールダウン兼ネガティブキャッシュ。`REMOTE_FOLLOW_SYNC_COOLDOWN`
/// （#229、フォロー一覧同期ジョブ自体の重複防止）とは別に、こちらは個々のactor URIの
/// 解決そのものが無条件・無制限に再投入されていた（2026-09-06実機確認: フォロー数の多い
/// リモートアクターのフォロー中/フォロワータブを開くたびに、404/410等で恒久的に解決できず
/// DBに登録されないままの数百〜数千URIが際限なく再enqueueされ、CPU・DBコネクションを
/// 食い尽くしてAPIが無応答になった）。
///
/// APIハンドラ層（`seiran-api`のAppState）とWorker層（`JobContext`）は別インスタンスで
/// 状態を共有しないため、プロセス内グローバルな`static`として持つ（`remote_follow_sync_recent`
/// のような各層のフィールドに分散させると、enqueue元ごとにクールダウンが独立してしまい
/// 効果が薄れる）。
const REMOTE_ACTOR_RESOLVE_COOLDOWN: Duration = Duration::from_secs(3600);

fn recent_map() -> &'static Mutex<HashMap<String, Instant>> {
    static MAP: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `Job::RemoteActorResolve`をenqueueしてよいか判定する。trueを返した場合のみ実際に
/// enqueueすること（呼び出しに成功した扱いで内部の直近時刻を更新するため、falseの場合に
/// 重ねて呼んでも次のクールダウン判定はリセットされない）。
pub fn should_enqueue(uri: &str) -> bool {
    let now = Instant::now();
    let mut map = recent_map().lock().expect("recent_map mutex poisoned");
    if let Some(last) = map.get(uri) {
        if now.duration_since(*last) < REMOTE_ACTOR_RESOLVE_COOLDOWN {
            return false;
        }
    }
    map.insert(uri.to_string(), now);
    true
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

pub async fn handle(uri: String, ctx: Arc<JobContext>) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!(
            "[RemoteActorResolve] DB pool 未設定のためスキップ (uri={})",
            uri
        );
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

    // uri が自ドメインを指す場合、リモート未知アクターではなくローカルユーザーの
    // Actor URI がフォロー一覧同期に混入しただけなので、fetch_actor へ進まず終了する
    // （進めると影の重複 fedi 行が生成される、#110）。
    let local_domain = ctx
        .inbox
        .as_ref()
        .map(|i| i.local_domain.as_str())
        .or_else(|| ctx.delivery.as_ref().map(|d| d.local_domain.as_str()));
    if let Some(local_domain) = local_domain {
        if crate::ap::extract_local_username(&uri, local_domain).is_some() {
            tracing::debug!("[RemoteActorResolve] 自ドメインURIのためスキップ: {}", uri);
            return Ok(());
        }
    }

    let domain = extract_domain(&uri);
    let sem = ctx.get_domain_semaphore(&domain).await;
    let _permit = sem
        .acquire_owned()
        .await
        .map_err(|e| format!("セマフォ取得失敗: {}", e))?;

    // Authorized Fetch（secure mode）対応。署名鍵が組み立てられない場合のみ未署名フェッチへ
    // フォールバックする。
    let actor = match ctx.system_signing_key() {
        Some((key_id, pem)) => ctx
            .ap_client
            .fetch_actor_signed(&uri, (&key_id, &pem))
            .await
            .map_err(|e| format!("アクタードキュメント取得失敗: {}", e))?,
        None => ctx
            .ap_client
            .fetch_actor(&uri)
            .await
            .map_err(|e| format!("アクタードキュメント取得失敗: {}", e))?,
    };

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
    let bio = actor
        .summary
        .as_deref()
        .map(crate::jobs::inbound_activity_process::strip_html);
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

    tracing::info!(
        "[RemoteActorResolve] 未知アクター解決完了: uri={} handle={}",
        uri,
        username
    );
    Ok(())
}

//! プロフィールの「別のアカウント」（alsoKnownAs、AP Moveの語彙をプロフィール表示・
//! 相互検証用途に転用したseiran独自拡張、`docs/protocols.md`参照）の相互検証ジョブ。
//!
//! `handlers::users::user_profile` がプロフィール表示のたびに積む。表示自体は常に
//! `actor_also_known_as.verified`（キャッシュ済みの前回検証結果）を読むだけで、この
//! ジョブが非同期でその値を更新する（次回表示時に反映される、「表示時再検証」パターン）。

use std::sync::Arc;

use crate::queue::worker::JobContext;
use crate::repository::{
    ActorRepository, AlsoKnownAsRepository, PgActorRepository, PgAlsoKnownAsRepository,
};

pub async fn handle(
    owner_actor_id: i64,
    target_actor_id: i64,
    ctx: Arc<JobContext>,
) -> Result<(), String> {
    let Some(pool) = &ctx.db_pool else {
        tracing::warn!("[AlsoKnownAsVerify] DB pool 未設定のためスキップ");
        return Ok(());
    };

    let actors = PgActorRepository::new(pool.clone());
    let also_known_as = PgAlsoKnownAsRepository::new(pool.clone());

    let Some(owner) = actors
        .find_by_id(owner_actor_id)
        .await
        .map_err(|e| format!("owner取得失敗: {}", e))?
    else {
        return Ok(());
    };
    let Some(target) = actors
        .find_by_id(target_actor_id)
        .await
        .map_err(|e| format!("target取得失敗: {}", e))?
    else {
        return Ok(());
    };

    let verified = match target.actor_type.as_str() {
        // Bskyの DID document が持つ alsoKnownAs はハンドル↔DID対応専用で、任意のURIを
        // 列挙する仕組みが無いため、検証対象外として常に false のままにする。
        "bsky" => false,
        "local" => also_known_as
            .is_listed_by(target_actor_id, owner_actor_id)
            .await
            .map_err(|e| format!("ローカル逆引き失敗: {}", e))?,
        _ => {
            let Some(target_ap_uri) = target.ap_uri.clone() else {
                return Ok(());
            };
            // owner はローカルアクター（プロフィール編集画面から登録）とリモートFediアクター
            // （`jobs::also_known_as_sync`がリモート本人のalsoKnownAs自己申告を取り込んだ場合）
            // の両方がありうる。リモートは`ap_uri`が既にDBにあるのでそれを使い、ローカルは
            // `ap_uri`がNULLのため自ドメインから組み立てる。
            let owner_ap_uri = match owner.ap_uri.clone() {
                Some(uri) => uri,
                None => {
                    let local_domain = ctx
                        .inbox
                        .as_ref()
                        .map(|i| i.local_domain.as_str())
                        .or_else(|| ctx.delivery.as_ref().map(|d| d.local_domain.as_str()));
                    let Some(local_domain) = local_domain else {
                        tracing::warn!("[AlsoKnownAsVerify] local_domain 未設定のためスキップ");
                        return Ok(());
                    };
                    format!("https://{}/users/{}", local_domain, owner.username)
                }
            };

            let domain = target_ap_uri.split('/').nth(2).unwrap_or("").to_string();
            let sem = ctx.get_domain_semaphore(&domain).await;
            let _permit = sem
                .acquire_owned()
                .await
                .map_err(|e| format!("セマフォ取得失敗: {}", e))?;
            match ctx.ap_client.fetch_actor(&target_ap_uri).await {
                Ok(target_ap) => target_ap.claims_also_known_as(&owner_ap_uri),
                Err(e) => {
                    tracing::info!(
                        "[AlsoKnownAsVerify] 移転先取得失敗のため未検証扱い: {} ({})",
                        target_ap_uri,
                        e
                    );
                    false
                }
            }
        }
    };

    also_known_as
        .set_verification(
            owner_actor_id,
            target_actor_id,
            verified,
            chrono::Utc::now(),
        )
        .await
        .map_err(|e| format!("検証結果保存失敗: {}", e))?;

    Ok(())
}

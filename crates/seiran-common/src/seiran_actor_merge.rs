//! リモートseiranアクターの相互申告マージ（#236）。
//!
//! AP経由・ATP経由どちらでアクターを発見しても、必ず1つの`actors`行
//! （`actor_type='remote_seiran'`）に収束させる。チャレンジ検証エンドポイントは持たず、
//! 相手側の実体（真正なap_uri/at_did）が既存行の自己申告と相互に一致した場合にのみ
//! 結婚（マージ）する。詳細は`docs/protocols.md` 11節参照。
//!
//! **既存行が見つかった場合は結婚ロジックを起動しない**（新規作成時のみ試みる）。
//! まだ複数のseiranサーバーが実運用されていないため、既に両側の行が別々に存在して
//! しまっている状態からの2行統合は扱わない、という方針による（同節参照）。

use crate::advisory_lock::{acquire_xact_lock_for_key, lock_class};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// `discover_fedi_actor`/`discover_bsky_actor`の結果。呼び出し側は`married`が`false`かつ
/// 申告（`claimed_at_did`/`claimed_ap_uri`）がある場合、相手を能動的に取りに行く
/// `Job::ActorMetadataResolve`をenqueueして結婚成立を早めるとよい（必須ではない）。
pub struct DiscoveryOutcome {
    pub actor_id: i64,
    /// 今回の呼び出しで新たに結婚（マージ）が成立したかどうか。
    pub married: bool,
}

/// AP経由でアクター`ap_uri`を発見した際の upsert + 相互一致マージ判定。
/// `claimed_at_did`はこのアクター自身がAP拡張フィールド（`seiranAtDid`）で自己申告した
/// ATP側の相手（未確認）。
#[allow(clippy::too_many_arguments)]
pub async fn discover_fedi_actor(
    pool: &PgPool,
    id: i64,
    ap_uri: &str,
    ap_inbox_url: &str,
    username: &str,
    domain: &str,
    display_name: &str,
    avatar_url: Option<&str>,
    bio: Option<&str>,
    emoji_map: &serde_json::Value,
    profile_fields: &serde_json::Value,
    claimed_at_did: Option<&str>,
    now: DateTime<Utc>,
) -> Result<DiscoveryOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    acquire_xact_lock_for_key(&mut tx, lock_class::ACTOR_MERGE, ap_uri).await?;

    let existing_id: Option<i64> = sqlx::query_scalar("SELECT id FROM actors WHERE ap_uri = $1")
        .bind(ap_uri)
        .fetch_optional(&mut *tx)
        .await?;

    let mut married = false;
    let actor_id = if let Some(existing_id) = existing_id {
        sqlx::query(
            "UPDATE actors SET ap_inbox_url = $2, display_name = $3, \
             avatar_url = COALESCE($4, avatar_url), bio = COALESCE($5, bio), \
             emoji_map = $6, profile_fields = $7, updated_at = $8, \
             claimed_at_did = COALESCE(claimed_at_did, $9) \
             WHERE id = $1",
        )
        .bind(existing_id)
        .bind(ap_inbox_url)
        .bind(display_name)
        .bind(avatar_url)
        .bind(bio)
        .bind(emoji_map)
        .bind(profile_fields)
        .bind(now)
        .bind(claimed_at_did)
        .execute(&mut *tx)
        .await?;
        existing_id
    } else {
        let married_id = match claimed_at_did {
            Some(did) => {
                let counterpart: Option<(i64, Option<String>)> = sqlx::query_as(
                    "SELECT id, claimed_ap_uri FROM actors WHERE at_did = $1",
                )
                .bind(did)
                .fetch_optional(&mut *tx)
                .await?;
                match counterpart {
                    Some((counterpart_id, Some(claimed))) if claimed == ap_uri => {
                        sqlx::query(
                            "UPDATE actors SET ap_uri = $2, ap_inbox_url = $3, \
                             actor_type = 'remote_seiran', claimed_ap_uri = NULL, \
                             display_name = $4, avatar_url = COALESCE($5, avatar_url), \
                             bio = COALESCE($6, bio), emoji_map = $7, profile_fields = $8, \
                             updated_at = $9 \
                             WHERE id = $1",
                        )
                        .bind(counterpart_id)
                        .bind(ap_uri)
                        .bind(ap_inbox_url)
                        .bind(display_name)
                        .bind(avatar_url)
                        .bind(bio)
                        .bind(emoji_map)
                        .bind(profile_fields)
                        .bind(now)
                        .execute(&mut *tx)
                        .await?;
                        tracing::info!(
                            "[seiran_actor_merge] 結婚成立（AP側発見）: actor_id={} ap_uri={} at_did={}",
                            counterpart_id,
                            ap_uri,
                            did
                        );
                        married = true;
                        Some(counterpart_id)
                    }
                    _ => None,
                }
            }
            None => None,
        };

        match married_id {
            Some(id) => id,
            None => {
                sqlx::query(
                    "INSERT INTO actors (id, actor_type, ap_uri, ap_inbox_url, username, domain, \
                     display_name, avatar_url, bio, created_at, updated_at, emoji_map, \
                     profile_fields, claimed_at_did) \
                     VALUES ($1, 'fedi', $2, $3, $4, $5, $6, $7, $8, $9, $9, $10, $11, $12)",
                )
                .bind(id)
                .bind(ap_uri)
                .bind(ap_inbox_url)
                .bind(username)
                .bind(domain)
                .bind(display_name)
                .bind(avatar_url)
                .bind(bio)
                .bind(now)
                .bind(emoji_map)
                .bind(profile_fields)
                .bind(claimed_at_did)
                .execute(&mut *tx)
                .await?;
                id
            }
        }
    };

    tx.commit().await?;
    Ok(DiscoveryOutcome { actor_id, married })
}

/// ATP経由でアクター`at_did`を発見した際の upsert + 相互一致マージ判定。
/// `claimed_ap_uri`はこのアクター自身がATP宣言レコード（`org.seiran.actor.declaration`）で
/// 自己申告したAP側の相手（未確認）。
#[allow(clippy::too_many_arguments)]
pub async fn discover_bsky_actor(
    pool: &PgPool,
    id: i64,
    at_did: &str,
    handle: &str,
    display_name: Option<&str>,
    avatar_url: Option<&str>,
    claimed_ap_uri: Option<&str>,
    now: DateTime<Utc>,
) -> Result<DiscoveryOutcome, sqlx::Error> {
    // fedi IDをロックキーに使う（DIDはローカルユーザーもドメイン未確定期間は
    // 持たず後から任意発行されうるため、常に先に確定するfedi IDの方を使う。
    // `docs/protocols.md` 11節参照）。自身の申告が無ければDID自体をキーにする
    // （相手を騙る余地は無い自分自身のDIDなので安全）。
    let mut tx = pool.begin().await?;
    let lock_key = claimed_ap_uri.unwrap_or(at_did);
    acquire_xact_lock_for_key(&mut tx, lock_class::ACTOR_MERGE, lock_key).await?;

    let existing_id: Option<i64> = sqlx::query_scalar("SELECT id FROM actors WHERE at_did = $1")
        .bind(at_did)
        .fetch_optional(&mut *tx)
        .await?;

    let mut married = false;
    let actor_id = if let Some(existing_id) = existing_id {
        sqlx::query(
            "UPDATE actors SET username = $2, \
             display_name = COALESCE($3, display_name), \
             avatar_url = COALESCE($4, avatar_url), updated_at = $5, \
             claimed_ap_uri = COALESCE(claimed_ap_uri, $6) \
             WHERE id = $1",
        )
        .bind(existing_id)
        .bind(handle)
        .bind(display_name)
        .bind(avatar_url)
        .bind(now)
        .bind(claimed_ap_uri)
        .execute(&mut *tx)
        .await?;
        existing_id
    } else {
        let married_id = match claimed_ap_uri {
            Some(uri) => {
                let counterpart: Option<(i64, Option<String>)> = sqlx::query_as(
                    "SELECT id, claimed_at_did FROM actors WHERE ap_uri = $1",
                )
                .bind(uri)
                .fetch_optional(&mut *tx)
                .await?;
                match counterpart {
                    Some((counterpart_id, Some(claimed))) if claimed == at_did => {
                        sqlx::query(
                            "UPDATE actors SET at_did = $2, actor_type = 'remote_seiran', \
                             claimed_at_did = NULL, \
                             display_name = COALESCE($3, display_name), \
                             avatar_url = COALESCE($4, avatar_url), updated_at = $5 \
                             WHERE id = $1",
                        )
                        .bind(counterpart_id)
                        .bind(at_did)
                        .bind(display_name)
                        .bind(avatar_url)
                        .bind(now)
                        .execute(&mut *tx)
                        .await?;
                        tracing::info!(
                            "[seiran_actor_merge] 結婚成立（ATP側発見）: actor_id={} ap_uri={} at_did={}",
                            counterpart_id,
                            uri,
                            at_did
                        );
                        married = true;
                        Some(counterpart_id)
                    }
                    _ => None,
                }
            }
            None => None,
        };

        match married_id {
            Some(id) => id,
            None => {
                sqlx::query(
                    "INSERT INTO actors (id, actor_type, at_did, username, domain, display_name, \
                     avatar_url, created_at, updated_at, claimed_ap_uri) \
                     VALUES ($1, 'bsky', $2, $3, '', $4, $5, $6, $6, $7)",
                )
                .bind(id)
                .bind(at_did)
                .bind(handle)
                .bind(display_name)
                .bind(avatar_url)
                .bind(now)
                .bind(claimed_ap_uri)
                .execute(&mut *tx)
                .await?;
                id
            }
        }
    };

    tx.commit().await?;
    Ok(DiscoveryOutcome { actor_id, married })
}

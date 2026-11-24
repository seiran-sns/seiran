//! リモートseiranアクターの相互申告マージ（#236）。
//!
//! AP経由・ATP経由どちらでアクターを発見しても、必ず1つの`actors`行
//! （`actor_type='remote_seiran'`）に収束させる。チャレンジ検証エンドポイントは持たず、
//! 相手側の実体（真正なap_uri/at_did）が既存行の自己申告と相互に一致した場合にのみ
//! 結婚（マージ）する。詳細は`docs/protocols.md` 11節参照。
//!
//! 既に両側の行が別々にある場合（Bluesky から seiran へ転入した人は、転入前の Bluesky 側の
//! 行が申告なしで残っている）は、既存行へ申告を書き足す時点で相互一致を確かめ、2行を
//! 統合する（`marry_existing_rows`。Fedi 側の行を残し、Bluesky 側の行を参照ごと吸収する）。
//!
//! AP経由発見とATP経由発見がほぼ同時に走った場合の直列化は、`pg_advisory_xact_lock`
//! ではなく`actors`の複合UNIQUE制約（`actors_mutual_claim_key`、マイグレーション
//! `20260906000000_actor_post_mutual_claim_unique.sql`参照）に委ねる。相互に申告し
//! 合っている2行は`(COALESCE(ap_uri, claimed_ap_uri), COALESCE(at_did, claimed_at_did))`
//! が同じ値に収束するため、片方をINSERT/UPDATEしようとした時点でUNIQUE制約違反になる。
//! `crate::unique_retry`でこれを検知しトランザクションの頭からやり直す（advisory lockだと
//! キーの不一致でデッドロックや直列化漏れが起きうる）。

use crate::repository::{BskyActorProfile, FediActorProfile};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::queue::worker::priority;
use crate::traits::{Job, JobQueue};

/// `discover_fedi_actor`/`discover_bsky_actor`の結果。
pub struct DiscoveryOutcome {
    pub actor_id: i64,
    /// 今回の呼び出しで新たに結婚（マージ）が成立したかどうか。
    pub married: bool,
}

/// `discover_fedi_actor`/`discover_bsky_actor`呼び出し後に必ず通す共通後処理。
/// 結婚成立時はJetstreamのwanted_dids再構築を促し（この行が初めて`at_did`を獲得した
/// 場合、Jetstream購読フィルタは自動追随しないため）、未成立でも
/// 自己申告（呼び出し元が`discover_*`へ渡したのと同じ`claimed_at_did`/`claimed_ap_uri`）が
/// あれば相手を能動的に取りに行く`Job::ActorMetadataResolve`をenqueueして結婚成立を
/// 早める（必須ではない、通常の受動的発見でも成立しうる）。
///
/// 入口（`discover_fedi_profile`/`discover_bsky_profile`）だけが呼ぶ。呼び出し元ごとに書くと
/// enqueueが漏れ、受動的に発見したアクターが能動フェッチされないまま孤立する。
async fn promote_after_discovery(
    pool: &PgPool,
    queue: &dyn JobQueue,
    outcome: &DiscoveryOutcome,
    claimed: Option<&str>,
) {
    if outcome.married {
        crate::jetstream_control::touch_jetstream_wanted_dids(pool).await;
    } else if claimed.is_some() {
        let _ = queue
            .enqueue(
                Job::ActorMetadataResolve {
                    actor_id: outcome.actor_id,
                },
                priority::LOW,
            )
            .await;
    }
}

/// AP経由でアクター`profile.ap_uri`を発見した際の upsert + 相互一致マージ判定。
/// `profile.claimed_at_did`はこのアクター自身がAP拡張フィールド（`seiranAtDid`）で自己申告した
/// ATP側の相手（未確認）。呼び出し元が別途知っている相手（自分自身の`at_did`等）を
/// 入れてはならない（自己参照で常に結婚が成立してしまう、`project_actor_merge_selfref_lessons`）。
/// 通常は `discover_fedi_profile` を使う（相手の解決ジョブの投入まで行うため）。これを直接
/// 呼ぶのは DB だけで完結させたい結合テスト。
pub async fn discover_fedi_actor(
    pool: &PgPool,
    id: i64,
    profile: &FediActorProfile,
    now: DateTime<Utc>,
) -> Result<DiscoveryOutcome, sqlx::Error> {
    crate::unique_retry::retry_on_unique_violation(|| async {
        discover_fedi_actor_once(pool, id, profile, now).await
    })
    .await
}

async fn discover_fedi_actor_once(
    pool: &PgPool,
    id: i64,
    profile: &FediActorProfile,
    now: DateTime<Utc>,
) -> Result<DiscoveryOutcome, sqlx::Error> {
    let ap_uri = profile.ap_uri.as_str();
    let claimed_at_did = profile.claimed_at_did.as_deref();
    let mut tx = pool.begin().await?;

    let existing: Option<(i64, String)> =
        sqlx::query_as("SELECT id, actor_type::text FROM actors WHERE ap_uri = $1")
            .bind(ap_uri)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((existing_id, kind)) = &existing {
        if kind == "local" {
            return Ok(DiscoveryOutcome {
                actor_id: *existing_id,
                married: false,
            });
        }
    }

    let mut married = false;
    let actor_id = if let Some((existing_id, kind)) = existing {
        // 申告を書き足すと Bluesky 側の既存行と相互に申告し合う形になるなら、先に2行を統合する
        // （書き足すだけだと `actors_mutual_claim_key` に当たる）。
        let counterpart = match claimed_at_did {
            Some(did) if kind != "remote_seiran" => {
                mutual_counterpart_by_did(&mut tx, existing_id, did, ap_uri).await?
            }
            _ => None,
        };
        if let Some(bsky_id) = counterpart {
            marry_existing_rows(&mut tx, existing_id, bsky_id).await?;
            married = true;
        }
        // 既に`remote_seiran`へ昇格済み（結婚成立済み）の行に対しては、`claimed_at_did`を
        // 復活させない。このUPDATEはINSERT直後に
        // 結婚成立した行へ、ほぼ同時に走っていた別経路の再発見（同じ相手を独立にAP/ATP
        // 両方からほぼ同時に発見した場合の「2番目」の到達）が無条件にCOALESCEで
        // `claimed_at_did`を書き戻してしまい、結婚済みなのに未確認の申告が残留する
        // （advisory lockはトランザクションを直列化するだけで、
        // 「既に結婚済みだから何もしない」という判断はしていないため、この保護がないと
        // 起きる）。
        sqlx::query(
            "UPDATE actors SET ap_inbox_url = $2, display_name = $3, \
             avatar_url = COALESCE($4, avatar_url), bio = COALESCE($5, bio), \
             emoji_map = $6, profile_fields = $7, updated_at = $8, \
             claimed_at_did = CASE WHEN actor_type = 'remote_seiran' THEN claimed_at_did \
                                    ELSE COALESCE(claimed_at_did, $9) END, \
             banner_url = COALESCE($10, banner_url) \
             WHERE id = $1",
        )
        .bind(existing_id)
        .bind(&profile.ap_inbox_url)
        .bind(&profile.display_name)
        .bind(&profile.avatar_url)
        .bind(&profile.bio)
        .bind(&profile.emoji_map)
        .bind(&profile.profile_fields)
        .bind(now)
        .bind(claimed_at_did)
        .bind(&profile.banner_url)
        .execute(&mut *tx)
        .await?;
        existing_id
    } else {
        let married_id = match claimed_at_did {
            Some(did) => {
                let counterpart: Option<(i64, Option<String>)> =
                    sqlx::query_as("SELECT id, claimed_ap_uri FROM actors WHERE at_did = $1")
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
                             updated_at = $9, banner_url = COALESCE($10, banner_url) \
                             WHERE id = $1",
                        )
                        .bind(counterpart_id)
                        .bind(ap_uri)
                        .bind(&profile.ap_inbox_url)
                        .bind(&profile.display_name)
                        .bind(&profile.avatar_url)
                        .bind(&profile.bio)
                        .bind(&profile.emoji_map)
                        .bind(&profile.profile_fields)
                        .bind(now)
                        .bind(&profile.banner_url)
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
                     profile_fields, claimed_at_did, banner_url) \
                     VALUES ($1, 'fedi', $2, $3, $4, $5, $6, $7, $8, $9, $9, $10, $11, $12, $13)",
                )
                .bind(id)
                .bind(ap_uri)
                .bind(&profile.ap_inbox_url)
                .bind(&profile.username)
                .bind(&profile.domain)
                .bind(&profile.display_name)
                .bind(&profile.avatar_url)
                .bind(&profile.bio)
                .bind(now)
                .bind(&profile.emoji_map)
                .bind(&profile.profile_fields)
                .bind(claimed_at_did)
                .bind(&profile.banner_url)
                .execute(&mut *tx)
                .await?;
                id
            }
        }
    };

    tx.commit().await?;
    Ok(DiscoveryOutcome { actor_id, married })
}

/// Bluesky 側の行を保存するときに、DID 側の宣言レコード（`org.seiran.actor.declaration`）を
/// 取りに行くか。
#[derive(Clone, Copy)]
pub enum DeclarationLookup {
    /// 常に取りに行く（1人のプロフィールを取り直す・相手を確かめる経路）。
    Always,
    /// 行がまだ無いときだけ取りに行く（検索結果・フォロワー一覧等、多数を一度に目にする経路）。
    /// 既存行の申告は前回の値を保つ。新しく作る行には必ず申告が付く。
    WhenNew,
}

/// Bluesky のアクターを保存する唯一の入口。DID 側の宣言レコード（AP 側の相手の自己申告）を
/// 取得し、相互申告マージ込みで保存し（`discover_bsky_actor`）、昇格処理
/// （`promote_after_discovery`）まで行う。Bluesky 側の行を作る・更新する経路は必ずこれを通す
/// （宣言レコードを取らずに行を作ると、その行は申告を持たず、相手と結婚できなくなる）。
///
/// `reenqueue_counterpart`が偽なら、未成立の自己申告を受けた相手側解決ジョブの再投入を抑止する
/// （相手側解決ジョブ自身から呼ぶ場合。無限ジョブ再投入を避けるため）。
/// 取得元は必ず DID 側の宣言レコードとする。呼び出し元が知っている相手（自分自身の
/// `ap_uri`等）を渡すと自己参照で常に結婚が成立してしまう（`project_actor_merge_selfref_lessons`）。
pub async fn discover_bsky_profile(
    pool: &PgPool,
    queue: &dyn JobQueue,
    profile: &BskyActorProfile<'_>,
    lookup: DeclarationLookup,
    reenqueue_counterpart: bool,
) -> Result<DiscoveryOutcome, sqlx::Error> {
    let fetch = match lookup {
        DeclarationLookup::Always => true,
        DeclarationLookup::WhenNew => !actor_exists_by_did(pool, profile.at_did).await?,
    };
    let claimed_ap_uri = if fetch {
        crate::atp::client::fetch_seiran_actor_declaration(profile.at_did).await
    } else {
        None
    };
    let now = Utc::now();
    let outcome = discover_bsky_actor(
        pool,
        crate::generate_snowflake_id(now),
        profile,
        claimed_ap_uri.as_deref(),
        now,
    )
    .await?;
    let claimed = claimed_ap_uri.as_deref().filter(|_| reenqueue_counterpart);
    promote_after_discovery(pool, queue, &outcome, claimed).await;
    Ok(outcome)
}

/// Fediverse のアクターを保存する唯一の入口。相互申告マージ込みで保存し
/// （`discover_fedi_actor`）、昇格処理（`promote_after_discovery`）まで行う。自己申告
/// （`seiranAtDid`）は Actor 文書に含まれるので、`profile.claimed_at_did` がそのまま使われる。
/// Fedi 側の行を作る・更新する経路は必ずこれを通す。
///
/// `reenqueue_counterpart` は `discover_bsky_profile` と同じ。
pub async fn discover_fedi_profile(
    pool: &PgPool,
    queue: &dyn JobQueue,
    profile: &FediActorProfile,
    reenqueue_counterpart: bool,
) -> Result<DiscoveryOutcome, sqlx::Error> {
    let now = Utc::now();
    let outcome =
        discover_fedi_actor(pool, crate::generate_snowflake_id(now), profile, now).await?;
    let claimed = profile
        .claimed_at_did
        .as_deref()
        .filter(|_| reenqueue_counterpart);
    promote_after_discovery(pool, queue, &outcome, claimed).await;
    Ok(outcome)
}

async fn actor_exists_by_did(pool: &PgPool, did: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM actors WHERE at_did = $1)")
        .bind(did)
        .fetch_one(pool)
        .await
}

/// ATP経由でアクター`at_did`を発見した際の upsert + 相互一致マージ判定。
/// `claimed_ap_uri`はこのアクター自身がATP宣言レコード（`org.seiran.actor.declaration`）で
/// 自己申告したAP側の相手（未確認）。
/// 通常は `discover_bsky_profile` を使う（宣言レコードの取得と相手の解決ジョブの投入まで行う
/// ため）。これを直接呼ぶのは DB だけで完結させたい結合テスト。
pub async fn discover_bsky_actor(
    pool: &PgPool,
    id: i64,
    profile: &BskyActorProfile<'_>,
    claimed_ap_uri: Option<&str>,
    now: DateTime<Utc>,
) -> Result<DiscoveryOutcome, sqlx::Error> {
    crate::unique_retry::retry_on_unique_violation(|| async {
        discover_bsky_actor_once(pool, id, profile, claimed_ap_uri, now).await
    })
    .await
}

async fn discover_bsky_actor_once(
    pool: &PgPool,
    id: i64,
    profile: &BskyActorProfile<'_>,
    claimed_ap_uri: Option<&str>,
    now: DateTime<Utc>,
) -> Result<DiscoveryOutcome, sqlx::Error> {
    let BskyActorProfile {
        at_did,
        handle,
        display_name,
        avatar_url,
        ..
    } = *profile;
    let mut tx = pool.begin().await?;

    let existing: Option<(i64, String)> =
        sqlx::query_as("SELECT id, actor_type::text FROM actors WHERE at_did = $1")
            .bind(at_did)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((existing_id, kind)) = &existing {
        if kind == "local" {
            return Ok(DiscoveryOutcome {
                actor_id: *existing_id,
                married: false,
            });
        }
    }

    let mut married = false;
    let counterpart = match (&existing, claimed_ap_uri) {
        (Some((existing_id, kind)), Some(uri)) if kind != "remote_seiran" => {
            mutual_counterpart_by_ap_uri(&mut tx, *existing_id, uri, at_did).await?
        }
        _ => None,
    };
    let actor_id = if let (Some((existing_id, _)), Some(fedi_id)) = (&existing, counterpart) {
        // 申告を書き足すと Fedi 側の既存行と相互に申告し合う形になる。Bluesky 側の行を
        // Fedi 側へ統合する（書き足すだけだと `actors_mutual_claim_key` に当たる）。
        marry_existing_rows(&mut tx, fedi_id, *existing_id).await?;
        sqlx::query("UPDATE actors SET at_handle = $2, updated_at = $3 WHERE id = $1")
            .bind(fedi_id)
            .bind(handle)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        married = true;
        fedi_id
    } else if let Some((existing_id, _)) = existing {
        // 既に`remote_seiran`へ昇格済み（結婚成立済み）の行に対しては、`username`を
        // ATPハンドル形式で上書きせず（結婚後の正式なusernameはFedi側由来のまま保つ）、
        // `claimed_ap_uri`も復活させない。理由は`discover_fedi_actor`の対称コメント参照。
        // `at_handle`はプロフィール画面のBsky ID表示専用の別列のため、`username`とは
        // 独立に`remote_seiran`でも常に最新値へ更新する。
        sqlx::query(
            "UPDATE actors SET username = CASE WHEN actor_type = 'remote_seiran' THEN username \
                                                ELSE $2 END, \
             at_handle = $2, \
             display_name = COALESCE($3, display_name), \
             avatar_url = COALESCE($4, avatar_url), updated_at = $5, \
             claimed_ap_uri = CASE WHEN actor_type = 'remote_seiran' THEN claimed_ap_uri \
                                    ELSE COALESCE(claimed_ap_uri, $6) END, \
             banner_url = COALESCE($7, banner_url) \
             WHERE id = $1",
        )
        .bind(existing_id)
        .bind(handle)
        .bind(display_name)
        .bind(avatar_url)
        .bind(now)
        .bind(claimed_ap_uri)
        .bind(profile.banner_url)
        .execute(&mut *tx)
        .await?;
        existing_id
    } else {
        let married_id = match claimed_ap_uri {
            Some(uri) => {
                let counterpart: Option<(i64, Option<String>)> =
                    sqlx::query_as("SELECT id, claimed_at_did FROM actors WHERE ap_uri = $1")
                        .bind(uri)
                        .fetch_optional(&mut *tx)
                        .await?;
                match counterpart {
                    Some((counterpart_id, Some(claimed))) if claimed == at_did => {
                        sqlx::query(
                            "UPDATE actors SET at_did = $2, at_handle = $6, actor_type = 'remote_seiran', \
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
                        .bind(handle)
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
                     avatar_url, at_handle, created_at, updated_at, claimed_ap_uri, banner_url) \
                     VALUES ($1, 'bsky', $2, $3, '', $4, $5, $3, $6, $6, $7, $8)",
                )
                .bind(id)
                .bind(at_did)
                .bind(handle)
                .bind(display_name)
                .bind(avatar_url)
                .bind(now)
                .bind(claimed_ap_uri)
                .bind(profile.banner_url)
                .execute(&mut *tx)
                .await?;
                id
            }
        }
    };

    tx.commit().await?;
    Ok(DiscoveryOutcome { actor_id, married })
}

/// `did` を持つ別の行が、`ap_uri` を申告していればその id（相互に申告し合う相手）。
async fn mutual_counterpart_by_did(
    tx: &mut sqlx::PgConnection,
    self_id: i64,
    did: &str,
    ap_uri: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM actors WHERE at_did = $1 AND id <> $2 AND claimed_ap_uri = $3 \
         AND actor_type = 'bsky'",
    )
    .bind(did)
    .bind(self_id)
    .bind(ap_uri)
    .fetch_optional(tx)
    .await
}

/// `ap_uri` を持つ別の行が、`did` を申告していればその id（相互に申告し合う相手）。
async fn mutual_counterpart_by_ap_uri(
    tx: &mut sqlx::PgConnection,
    self_id: i64,
    ap_uri: &str,
    did: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM actors WHERE ap_uri = $1 AND id <> $2 AND claimed_at_did = $3 \
         AND actor_type = 'fedi'",
    )
    .bind(ap_uri)
    .bind(self_id)
    .bind(did)
    .fetch_optional(tx)
    .await
}

/// 相互に申告し合う既存の2行を1行にする。Fedi 側の行（結婚後の正式な username を持つ）を残し、
/// Bluesky 側の行を参照ごと吸収して、残った行に `at_did` を移して `remote_seiran` にする。
async fn marry_existing_rows(
    tx: &mut sqlx::PgConnection,
    fedi_id: i64,
    bsky_id: i64,
) -> Result<(), sqlx::Error> {
    let absorbed = crate::repository::actor_merge::absorb_actor(tx, fedi_id, bsky_id).await?;
    let (at_did, at_handle) = absorbed
        .map(|a| (a.at_did, a.at_handle))
        .unwrap_or_default();
    sqlx::query(
        "UPDATE actors SET at_did = $2, at_handle = COALESCE($3, at_handle), \
         actor_type = 'remote_seiran', claimed_at_did = NULL, claimed_ap_uri = NULL \
         WHERE id = $1",
    )
    .bind(fedi_id)
    .bind(at_did)
    .bind(at_handle)
    .execute(&mut *tx)
    .await?;
    tracing::info!(
        "[seiran_actor_merge] 既存の2行を統合: 残す actor_id={} 吸収した actor_id={}",
        fedi_id,
        bsky_id
    );
    Ok(())
}

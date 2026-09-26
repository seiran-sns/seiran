//! アカウント単位PLCローテーションキーへのバックフィル（転出元API対応の前提、Phase A）。
//!
//! 対象は「seiranが自前でジェネシスDIDを発行したローカルアカウント」のみ。既存DID転入
//! （`at_migration_requests`に`actor_id`が入っている行）で作成されたアカウントは対象外
//! ——このバックフィルはseiranが**現在有効な鍵**（サーバー共有鍵）で署名して更新オペレー
//! ションを提出する仕組みだが、転入済みアカウントの現在有効な鍵は転入元PDS発行のもの
//! （例: Bluesky側）であり、seiranはその秘密鍵を持っていない。共有鍵で署名しても
//! plc.directoryには正当な鍵として拒否されるため、この機構では対応できない
//! （`handlers::migration::submit_plc_token`が転入完了時点でseiran発行の専用鍵を
//! 持たせるため、新規の転入済みアカウントはこの制約を持たない）。
//!
//! ジョブ本体（`jobs::rotation_key_backfill`）はグローバルな単一advisory lockで排他される
//! ため、複数ワーカーによる同時claimは発生しない。`FOR UPDATE SKIP LOCKED`は不要。

use async_trait::async_trait;
use sqlx::PgPool;

#[async_trait]
pub trait RotationKeyBackfillRepository: Send + Sync {
    /// 未バックフィルの次の1件を返す（`(actor_id, at_did)`）。`after_id`より大きいIDの
    /// うち最小のものを返す（`None`なら先頭から）。dry-runでは`mark_backfilled`を呼ばず
    /// 状態を進めないため、呼び出し側がこのカーソルで進行を管理する（呼ばないと同じ
    /// 1件を無限に返し続けてしまう）。無ければ`None`。
    async fn next_candidate(
        &self,
        after_id: Option<i64>,
    ) -> Result<Option<(i64, String)>, sqlx::Error>;

    /// バックフィル完了をマークする。
    async fn mark_backfilled(
        &self,
        actor_id: i64,
        at_rotation_key_pem: &str,
    ) -> Result<(), sqlx::Error>;

    /// 残り件数（dry-run表示・進捗確認用）。
    async fn count_pending(&self) -> Result<i64, sqlx::Error>;
}

pub struct PgRotationKeyBackfillRepository {
    pool: PgPool,
}

impl PgRotationKeyBackfillRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// 対象を「ジェネシス作成の、現在も有効なローカルアカウントのみ」に絞るWHERE句。
/// 退会済み（`withdrawn_at`設定済み）アカウントは対象外——`withdraw`はDIDを物理削除せず
/// `actors`行を残すため対象クエリには含まれてしまうが、既に退会したDIDのPLC状態は
/// 提出時点で想定と食い違いうる（実機で発見: "Operations not correctly ordered"）。
const CANDIDATE_WHERE: &str = "actor_type = 'local'
     AND at_did IS NOT NULL
     AND at_rotation_key_pem IS NULL
     AND withdrawn_at IS NULL
     AND id NOT IN (SELECT actor_id FROM at_migration_requests WHERE actor_id IS NOT NULL)";

#[async_trait]
impl RotationKeyBackfillRepository for PgRotationKeyBackfillRepository {
    async fn next_candidate(
        &self,
        after_id: Option<i64>,
    ) -> Result<Option<(i64, String)>, sqlx::Error> {
        sqlx::query_as(&format!(
            "SELECT id, at_did FROM actors WHERE {CANDIDATE_WHERE} AND id > $1 ORDER BY id LIMIT 1"
        ))
        .bind(after_id.unwrap_or(0))
        .fetch_optional(&self.pool)
        .await
    }

    async fn mark_backfilled(
        &self,
        actor_id: i64,
        at_rotation_key_pem: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE actors SET at_rotation_key_pem = $1 WHERE id = $2")
            .bind(at_rotation_key_pem)
            .bind(actor_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn count_pending(&self) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM actors WHERE {CANDIDATE_WHERE}"
        ))
        .fetch_one(&self.pool)
        .await
    }
}

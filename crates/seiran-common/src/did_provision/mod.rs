//! 新規ローカルアカウントの DID 発行（既存 DID の転入は `migration`）。
//!
//! 登録時は DID をローカル計算で確定して「DNS TXT 登録完了待ち」（`pending_did_provisions`
//! の行）として保存するだけで、`actors.at_did` は NULL（DID 未作成と同じ扱い）のまま返す。
//! その後 `advance` が行を1段ずつ進める:
//!
//! 1. Cloudflare に `_atproto.{handle}` TXT を作る
//! 2. ゾーンの全権威 NS に TXT が見えるのを待ち、見えてから `TXT_PUBLISH_GRACE` 置く
//! 3. plc.directory へ genesis を送り、DID と鍵を `actors` に移して、初期レコードと
//!    `#identity` を firehose へ出す
//!
//! TXT が権威 NS に公開される前に最初のイベントを出すと、Relay/AppView が NXDOMAIN を受けて
//! ハンドルを無効と記録し、長時間直らない（AppView は commit でもハンドルを解決し、一度無効に
//! なると commit では1時間再検証しない）。そのため最初のイベントより前、PLC 送信の時点から
//! 待つ。Cloudflare が未設定（途中で設定が消された場合を含む）なら TXT は作れないので、
//! 1・2 を飛ばして 3 へ進む。
//!
//! 公開までの時間に上限は無い（Cloudflare の障害中など）ので、登録リクエストでは1段だけ
//! 進めて返す。残りは登録直後のバックグラウンドタスク（`kick`）が間を置かずに進め、それでも
//! 終わらなければ定時処理（`seiran_atp_repo::did_provision_poll`）が `run_pending` で進める。

use std::sync::Arc;

use chrono::Utc;
use sqlx::PgPool;

use crate::atp::plc::{plc_did_exists, submit_plc_genesis_op};
use crate::atp::service::ProfileCommit;
use crate::atp::{prepare_plc_genesis, signing_key_from_pem, AtpCommitService, PlcGenesis};
use crate::cloudflare::{client_from_settings, CloudflareClient, TXT_PUBLISH_GRACE};
use crate::repository::did_provision::{self as repo, NewPendingDid, PendingDidRow};
use crate::repository::SiteSettingsRepository;
use crate::traits::JobQueue;
use crate::LocalDomain;

pub mod migration;

/// 1つの行を処理中として押さえておく時間。PLC・Cloudflare への通信が終わるのに十分な長さで、
/// プロセスが途中で止まったときに次の処理が引き継げるまでの時間でもある。
const LEASE_SECS: i64 = 60;
/// 登録直後に定時処理の外で進める間隔と期間（`kick`）。
const KICK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
const KICK_DURATION: std::time::Duration = std::time::Duration::from_secs(180);

fn grace() -> chrono::Duration {
    chrono::Duration::from_std(TXT_PUBLISH_GRACE).unwrap_or_default()
}

pub struct DidProvisioner {
    pub pool: PgPool,
    pub http: Arc<reqwest::Client>,
    pub atp_service: Arc<AtpCommitService>,
    pub site_settings: Arc<dyn SiteSettingsRepository>,
    pub local_domain: LocalDomain,
    /// サーバー共有の recovery 用ローテーション鍵（`secrets.toml` の `atproto_private_key_pem`）。
    pub server_rotation_key_pem: String,
    /// 転入の DID 確定後に取り込みジョブを積む先。
    pub job_queue: Arc<dyn JobQueue>,
}

/// `pending_did_provisions`・`awaiting_dns_txt` の転入で共通の、TXT の進み具合を1段進めた結果。
pub(crate) enum TxtProgress {
    /// TXT を作った（レコード ID を保存する）。
    Created(String),
    /// 全権威 NS で公開を確認した（確認時刻を保存する）。
    Visible,
    /// まだ公開されていないか、確認から `TXT_PUBLISH_GRACE` 経っていない。
    Waiting,
    /// PLC へ送ってよい。
    Ready,
}

pub(crate) async fn txt_progress(
    cf: &CloudflareClient,
    handle: &str,
    did: &str,
    record_id: Option<&str>,
    visible_at: Option<chrono::DateTime<Utc>>,
) -> Result<TxtProgress, String> {
    match (record_id, visible_at) {
        (None, _) => {
            let record_id = cf
                .set_atproto_txt(handle, did)
                .await
                .map_err(|e| format!("TXT 作成に失敗: {e}"))?;
            tracing::info!("[did_provision] TXT 作成: _atproto.{}", handle);
            Ok(TxtProgress::Created(record_id))
        }
        (Some(_), None) => {
            let published = cf
                .is_atproto_txt_published(handle, did)
                .await
                .map_err(|e| format!("TXT 公開確認に失敗: {e}"))?;
            if published {
                tracing::info!("[did_provision] TXT 公開を確認: _atproto.{}", handle);
                Ok(TxtProgress::Visible)
            } else {
                Ok(TxtProgress::Waiting)
            }
        }
        (Some(_), Some(at)) if Utc::now() - at < grace() => Ok(TxtProgress::Waiting),
        (Some(_), Some(_)) => Ok(TxtProgress::Ready),
    }
}

/// `advance` が「まだ待ち」（`true`）を返す間、`KICK_INTERVAL` ごとに呼び続ける。
/// 定時処理の間隔ぶんの遅れを無くすため。`KICK_DURATION` を過ぎても待ちなら定時処理に任せる。
fn spawn_kick<F, Fut>(advance: F)
where
    F: Fn() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = bool> + Send,
{
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + KICK_DURATION;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(KICK_INTERVAL).await;
            if !advance().await {
                return;
            }
        }
    });
}

/// ローカル計算で確定した DID と、登録時に保存する genesis op。
pub struct PreparedDid {
    genesis: PlcGenesis,
    genesis_op: serde_json::Value,
}

impl PreparedDid {
    pub fn as_new_pending(&self) -> NewPendingDid<'_> {
        NewPendingDid {
            at_did: &self.genesis.did,
            at_signing_key_pem: &self.genesis.signing_key_pem,
            at_rotation_key_pem: &self.genesis.rotation_key_pem,
            genesis_op: &self.genesis_op,
        }
    }
}

impl DidProvisioner {
    /// 新規アカウントの DID を確定する（通信なし）。自ホストドメインが未確定
    /// （シングルホストモード）の間は DID を作らないので `None`。
    pub fn prepare(&self, username: &str) -> Result<Option<PreparedDid>, String> {
        if !self.local_domain.is_confirmed() {
            return Ok(None);
        }
        let rotation_key =
            signing_key_from_pem(&self.server_rotation_key_pem).map_err(|e| e.to_string())?;
        let genesis = prepare_plc_genesis(username, &self.local_domain, &rotation_key)
            .map_err(|e| e.to_string())?;
        let genesis_op = genesis.signed_op_json().map_err(|e| e.to_string())?;
        Ok(Some(PreparedDid {
            genesis,
            genesis_op,
        }))
    }

    /// 「DNS TXT 登録完了待ち」の全アカウントと転入を1段ずつ進める（定時処理）。
    pub async fn run_pending(&self) {
        // 別プロセスのセットアップで確定したドメインは再起動まで見えない。未確定のまま進めると
        // ハンドルを組み立てられないので待つ。
        if !self.local_domain.is_confirmed() {
            return;
        }
        let ids = match repo::list_unlocked_ids(&self.pool).await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::error!("[did_provision] 待ちアカウントの取得に失敗: {}", e);
                return;
            }
        };
        for actor_id in ids {
            self.advance(actor_id).await;
        }
        self.run_pending_migrations().await;
    }

    /// 1つのアカウントを1段進め、まだ待ち（DID 未確定）なら `true` を返す。他が処理中なら
    /// 何もしない。失敗はログだけ出し、次回に同じ段からやり直す。
    pub async fn advance(&self, actor_id: i64) -> bool {
        let row = match repo::claim(&self.pool, actor_id, LEASE_SECS).await {
            Ok(Some(row)) => row,
            // 他が処理中か、確定済みで行が無い。
            Ok(None) => return repo::exists(&self.pool, actor_id).await.unwrap_or(true),
            Err(e) => {
                tracing::error!("[did_provision] actor_id={} の取得に失敗: {}", actor_id, e);
                return true;
            }
        };
        let pending = match self.step(&row).await {
            Ok(finalized) => !finalized,
            Err(e) => {
                tracing::warn!(
                    "[did_provision] actor_id={} did={}: {}",
                    actor_id,
                    row.at_did,
                    e
                );
                true
            }
        };
        // 確定済みなら行は消えており、何も更新しない。
        if let Err(e) = repo::release(&self.pool, actor_id).await {
            tracing::error!("[did_provision] actor_id={} の解放に失敗: {}", actor_id, e);
        }
        pending
    }

    /// 定時処理を待たずに、このアカウントを進め続ける（登録直後用、`spawn_kick`）。
    pub fn kick(self: &Arc<Self>, actor_id: i64) {
        let this = Arc::clone(self);
        spawn_kick(move || {
            let this = Arc::clone(&this);
            async move { this.advance(actor_id).await }
        });
    }

    /// 1段進める。DID を確定したら `true`。
    async fn step(&self, row: &PendingDidRow) -> Result<bool, String> {
        let handle = format!(
            "{}.{}",
            crate::username::to_atp_username(&row.username),
            self.local_domain
        );
        let cf = client_from_settings(self.site_settings.as_ref(), &self.http).await;
        if let Some(cf) = &cf {
            let progress = txt_progress(
                cf,
                &handle,
                &row.at_did,
                row.cf_txt_record_id.as_deref(),
                row.txt_visible_at,
            )
            .await?;
            match progress {
                TxtProgress::Created(record_id) => {
                    repo::set_txt_record_id(&self.pool, row.actor_id, &record_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    return Ok(false);
                }
                TxtProgress::Visible => {
                    repo::mark_txt_visible(&self.pool, row.actor_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    return Ok(false);
                }
                TxtProgress::Waiting => return Ok(false),
                TxtProgress::Ready => {}
            }
        }
        self.complete(row, cf.as_deref(), &handle).await?;
        Ok(true)
    }

    /// plc.directory へ genesis を送り、DID と鍵を `actors` に移して初期レコードを出す。
    async fn complete(
        &self,
        row: &PendingDidRow,
        cf: Option<&CloudflareClient>,
        handle: &str,
    ) -> Result<(), String> {
        let registered = plc_did_exists(&row.at_did, &self.http)
            .await
            .map_err(|e| format!("plc.directory の確認に失敗: {e}"))?;
        if !registered {
            if let Err(e) = submit_plc_genesis_op(&row.at_did, &row.genesis_op, &self.http).await {
                // 同じ genesis を送り直しても同じ理由で失敗し続けることがあるため、鍵ごと
                // 作り直す（DID も変わるので TXT も作り直しになる）。
                self.regenerate(row, cf, handle).await?;
                return Err(format!("did:plc 送信に失敗（鍵を作り直して再試行）: {e}"));
            }
        }
        repo::finalize(&self.pool, row)
            .await
            .map_err(|e| format!("DID の確定に失敗: {e}"))?;
        tracing::info!("[did_provision] DID 確定: {} ({})", row.at_did, handle);
        self.publish_initial_atp_records(row.actor_id, &row.username, &row.at_did)
            .await;
        Ok(())
    }

    async fn regenerate(
        &self,
        row: &PendingDidRow,
        cf: Option<&CloudflareClient>,
        handle: &str,
    ) -> Result<(), String> {
        let prepared = self
            .prepare(&row.username)?
            .ok_or_else(|| "自ホストドメインが未確定".to_string())?;
        if let (Some(cf), Some(record_id)) = (cf, row.cf_txt_record_id.as_deref()) {
            if let Err(e) = cf.delete_txt_record(record_id).await {
                tracing::warn!(
                    "[did_provision] 旧 TXT の削除に失敗: _atproto.{}: {}",
                    handle,
                    e
                );
            }
        }
        repo::replace_genesis(&self.pool, row.actor_id, &prepared.as_new_pending())
            .await
            .map_err(|e| e.to_string())
    }

    /// 確定したアカウントの ATP リポジトリへ初期レコードをコミットし、`#identity` を送る。
    /// 失敗してもアカウント自体は確定済みのためログのみ（起動時のバックフィルが補う）。
    async fn publish_initial_atp_records(&self, actor_id: i64, username: &str, at_did: &str) {
        let now = Utc::now();
        let atp = &self.atp_service;
        if let Err(e) = atp
            .commit_profile(actor_id, &ProfileCommit::new(username), now)
            .await
        {
            tracing::error!("[did_provision] ATP プロフィールコミット失敗: {}", e);
        }
        // Bsky公式クライアントからのDM受信を許可する設定（`docs/protocols.md` 9節）。
        // 無いとBluesky公式クライアントが相手（このユーザー）へのDM送信を保守的にブロックする。
        if let Err(e) = atp.commit_chat_declaration(actor_id, now).await {
            tracing::error!("[did_provision] chat declaration コミット失敗: {}", e);
        }
        // リモートseiranアクターの相互申告マージ用の自己申告（#236）。
        let ap_actor_uri = format!("https://{}/users/{}", self.local_domain, username);
        if let Err(e) = atp
            .commit_seiran_actor_declaration(actor_id, &ap_actor_uri, now)
            .await
        {
            tracing::error!(
                "[did_provision] seiran actor declaration コミット失敗: {}",
                e
            );
        }
        // #identity フレームを Relay に送信して AppView の handle キャッシュを更新させる。
        // commit_profile より後に送信することで seq 順序が保たれる。
        let handle = format!(
            "{}.{}",
            crate::username::to_atp_username(username),
            self.local_domain
        );
        if let Err(e) = atp
            .broadcast_identity_event(actor_id, at_did, &handle, now)
            .await
        {
            tracing::error!("[did_provision] #identity broadcast 失敗: {}", e);
        }
    }
}

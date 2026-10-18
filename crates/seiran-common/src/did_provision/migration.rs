//! 既存 DID 転入の「DNS TXT 登録完了待ち」（`at_migration_requests.status = 'awaiting_dns_txt'`）。
//!
//! `submit-plc-token` は転入元 PDS に PLC 操作へ署名させ（`signPlcOperation`、メールの
//! ワンタイムトークンを消費する）、署名済みの操作と新しい鍵を保存してこの状態にする。
//! 署名済みの操作は、間に他の操作が入らない限り後から送っても有効なので、新規アカウントと
//! 同じく TXT が全権威 NS に公開されたのを確かめてから plc.directory へ送り、アカウントを
//! 確定して取り込みジョブを積む。PLC 送信でハンドルが切り替わった時点で Relay/AppView が
//! 解決しにくるため、送信前に待つ必要がある。

use std::sync::Arc;

use chrono::Utc;
use sqlx::PgPool;

use super::{spawn_kick, txt_progress, DidProvisioner, TxtProgress, LEASE_SECS};
use crate::atp::migration_client::submit_plc_operation;
use crate::atp::PlcError;
use crate::cloudflare::client_from_settings;
use crate::queue::worker::priority;
use crate::repository::at_migration::{self as repo, MigrationDnsStage};
use crate::repository::{
    ActorRepository, AtMigrationRepository, AtMigrationRequestRow, NewLocalActor,
    PgActorRepository, PgAtMigrationRepository, PgUserRepository, UserRepository,
};
use crate::traits::Job;
use crate::LocalDomain;

impl DidProvisioner {
    pub(super) async fn run_pending_migrations(&self) {
        let ids = match repo::list_dns_wait_ids(&self.pool).await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::error!("[did_provision] 待ち転入の取得に失敗: {}", e);
                return;
            }
        };
        for id in ids {
            self.advance_migration(id).await;
        }
    }

    /// 1つの転入を1段進め、まだ `awaiting_dns_txt` なら `true` を返す。
    pub async fn advance_migration(&self, id: i64) -> bool {
        let stage = match repo::claim_dns_wait(&self.pool, id, LEASE_SECS).await {
            Ok(Some(stage)) => stage,
            Ok(None) => return self.is_awaiting_dns_txt(id).await,
            Err(e) => {
                tracing::error!("[did_provision] 転入 id={} の取得に失敗: {}", id, e);
                return true;
            }
        };
        let pending = match self.migration_step(&stage).await {
            Ok(done) => !done,
            Err(e) => {
                tracing::warn!("[did_provision] 転入 id={}: {}", id, e);
                true
            }
        };
        if let Err(e) = repo::release_dns_wait(&self.pool, id).await {
            tracing::error!("[did_provision] 転入 id={} の解放に失敗: {}", id, e);
        }
        pending
    }

    /// 定時処理を待たずに、この転入を進め続ける（`submit-plc-token` 直後用）。
    pub fn kick_migration(self: &Arc<Self>, id: i64) {
        let this = Arc::clone(self);
        spawn_kick(move || {
            let this = Arc::clone(&this);
            async move { this.advance_migration(id).await }
        });
    }

    async fn is_awaiting_dns_txt(&self, id: i64) -> bool {
        match PgAtMigrationRepository::new(self.pool.clone())
            .get(id)
            .await
        {
            Ok(Some(req)) => req.status == "awaiting_dns_txt",
            Ok(None) => false,
            Err(_) => true,
        }
    }

    /// 1段進める。`awaiting_dns_txt` を抜けたら（確定・失敗・打ち切り）`true`。
    async fn migration_step(&self, stage: &MigrationDnsStage) -> Result<bool, String> {
        let req = PgAtMigrationRepository::new(self.pool.clone())
            .get(stage.id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "転入リクエストが見つかりません".to_string())?;
        let handle = format!(
            "{}.{}",
            crate::username::to_atp_username(&req.new_username),
            self.local_domain
        );
        if let Some(cf) = client_from_settings(self.site_settings.as_ref(), &self.http).await {
            let progress = txt_progress(
                &cf,
                &handle,
                &req.source_did,
                stage.cf_txt_record_id.as_deref(),
                stage.txt_visible_at,
            )
            .await?;
            match progress {
                TxtProgress::Created(record_id) => {
                    repo::set_dns_txt_record_id(&self.pool, stage.id, &record_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    return Ok(false);
                }
                TxtProgress::Visible => {
                    repo::mark_dns_txt_visible(&self.pool, stage.id)
                        .await
                        .map_err(|e| e.to_string())?;
                    return Ok(false);
                }
                TxtProgress::Waiting => return Ok(false),
                TxtProgress::Ready => {}
            }
        }
        self.complete_migration(&req, stage).await?;
        Ok(true)
    }

    /// 保存しておいた PLC 操作を送り、アカウントを確定して取り込みジョブを積む。
    async fn complete_migration(
        &self,
        req: &AtMigrationRequestRow,
        stage: &MigrationDnsStage,
    ) -> Result<(), String> {
        let (Some(operation), Some(signing_key_pem), Some(rotation_key_pem)) = (
            stage.staged_plc_operation.as_ref(),
            stage.staged_signing_key_pem.as_deref(),
            stage.staged_rotation_key_pem.as_deref(),
        ) else {
            return Err("保存済みの PLC 操作がありません".to_string());
        };
        // 打ち切り（`awaiting_dns_txt` → `abandoned`）と競合したら打ち切りを優先する。
        if !repo::transition_status(&self.pool, req.id, "awaiting_dns_txt", "submitting_plc")
            .await
            .map_err(|e| e.to_string())?
        {
            return Ok(());
        }
        let migrations = PgAtMigrationRepository::new(self.pool.clone());
        if let Err(e) = submit_plc_operation(&req.source_did, operation, &self.http).await {
            // plc.directory が操作を拒んだ（転入元で別の操作が入った等）なら送り直しても通らない。
            // 通信の失敗なら待ちに戻して次回送り直す。
            let rejected =
                matches!(&e, PlcError::PlcDirectory { status, .. } if (400..500).contains(status));
            if rejected {
                let message = format!("PLC 操作が拒否されました: {e}");
                migrations
                    .set_failed(req.id, "failed", &message, Utc::now())
                    .await
                    .map_err(|e| e.to_string())?;
            } else {
                repo::transition_status(&self.pool, req.id, "submitting_plc", "awaiting_dns_txt")
                    .await
                    .map_err(|e| e.to_string())?;
            }
            return Err(format!("submitPlcOperation に失敗: {e}"));
        }
        // ★不可逆境界: DID の service endpoint は seiran を指すようになった。
        migrations
            .mark_plc_submitted(req.id, signing_key_pem, rotation_key_pem, Utc::now())
            .await
            .map_err(|e| format!("mark_plc_submitted に失敗（PLC は提出済み！）: {e}"))?;
        let email = req
            .email
            .as_deref()
            .ok_or_else(|| "メールアドレス未確定".to_string())?;
        // ここで失敗しても `submitting_plc`・鍵記録済みなので、`submit-plc-token` の再入で
        // アカウント作成だけをやり直せる。
        let (user_id, actor_id) = materialize_local_account(
            &self.pool,
            &self.local_domain,
            req,
            email,
            signing_key_pem,
            rotation_key_pem,
        )
        .await?;
        if let Err(e) = migrations
            .confirm_account_created(req.id, actor_id, user_id, Utc::now())
            .await
        {
            tracing::error!(
                "[did_provision] 転入 id={} の確定記録に失敗（アカウントは作成済み）: {}",
                req.id,
                e
            );
        }
        tracing::info!(
            "[did_provision] 転入の PLC 送信・アカウント確定: {} ({})",
            req.source_did,
            req.new_username
        );
        if let Err(e) = self
            .job_queue
            .enqueue(
                Job::MigrationImportProcess { request_id: req.id },
                priority::LOW,
            )
            .await
        {
            tracing::error!(
                "[did_provision] MigrationImportProcess enqueue 失敗 (request_id={}): {}",
                req.id,
                e
            );
        }
        Ok(())
    }
}

/// 転入したアカウントの `users`/`actors` 行を確定し、`(user_id, actor_id)` を返す。
///
/// 転入元 DID が既に `actors` にリモートキャッシュ行として存在することがある（firehose 購読や
/// プロフィール参照で自然に発生し、`start` 時点の重複チェックは `actor_type='local'` だけを
/// 見るため素通りする）。あればローカル用に変換し、無ければ新規作成する。
pub async fn materialize_local_account(
    pool: &PgPool,
    local_domain: &LocalDomain,
    req: &AtMigrationRequestRow,
    email: &str,
    signing_key_pem: &str,
    rotation_key_pem: &str,
) -> Result<(i64, i64), String> {
    let actors = PgActorRepository::new(pool.clone());
    let existing_actor_id = actors
        .find_by_did(&req.source_did)
        .await
        .map_err(|e| e.to_string())?
        .map(|a| a.id);
    let existing_user_id = PgUserRepository::new(pool.clone())
        .find_id_by_email(email)
        .await
        .map_err(|e| e.to_string())?;

    let user_id = match existing_user_id {
        Some(uid) => uid,
        None => crate::repository::insert_local_user(
            pool,
            email,
            &req.password_hash,
            "user",
            req.agreed_tos_text.as_deref(),
        )
        .await
        .map_err(|e| format!("users INSERT 失敗: {e}"))?,
    };

    let actor_id = if let Some(existing_id) = existing_actor_id {
        let ap_uri = format!("https://{}/users/{}", local_domain, req.new_username);
        crate::repository::actor::convert_remote_to_local(
            pool,
            &crate::repository::actor::ConvertToLocal {
                actor_id: existing_id,
                user_id,
                username: &req.new_username,
                domain: local_domain.as_str(),
                ap_uri: &ap_uri,
                signing_key_pem,
                rotation_key_pem,
            },
        )
        .await
        .map_err(|e| format!("actors UPDATE（リモート→ローカル変換）失敗: {e}"))?;
        existing_id
    } else {
        let new_id = crate::generate_snowflake_id(Utc::now());
        actors
            .insert_local(
                user_id,
                &NewLocalActor {
                    id: new_id,
                    username: &req.new_username,
                    domain: local_domain,
                    at_did: Some(&req.source_did),
                    at_signing_key_pem: Some(signing_key_pem),
                    at_rotation_key_pem: Some(rotation_key_pem),
                    birth_date: None,
                },
            )
            .await
            .map_err(|e| format!("actors INSERT 失敗: {e}"))?;
        new_id
    };
    Ok((user_id, actor_id))
}

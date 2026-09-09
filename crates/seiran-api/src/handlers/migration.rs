//! 既存DID転入フロー（`docs/account_migration.md`）のエントリーポイント。
//!
//! `submitting_plc`成功までアカウント（`users`/`actors`）が存在しないため、通常のJWT認証は
//! 使えない。`password_reset`と同様、ランダムトークンをSHA-256ハッシュ化してDB保存し、
//! レスポンス一回きりで生値を返す方式（`X-Migration-Token`ヘッダで以降の操作を認可する、
//! 本ファイルでは`start`のみ実装——ステータス確認・トークン入力等は後続フェーズで追加）。

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use seiran_common::repository::{AtMigrationRepository, AtMigrationRequestRow, PgAtMigrationRepository};
use seiran_common::LocalAuthProvider;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::ApiError;
use crate::handlers::auth::{AuthResponse, UserInfo};
use crate::AppState;

/// `X-Migration-Token`ヘッダを検証し、対応するリクエスト行を返す。
/// `submitting_plc`成功までアカウントが存在しないため、通常のJWT認証の代わりにこれを使う
/// （ファイル冒頭のコメント参照）。
async fn authorize_migration_request(
    state: &AppState,
    id: i64,
    headers: &HeaderMap,
) -> Result<AtMigrationRequestRow, ApiError> {
    let token = headers
        .get("X-Migration-Token")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .ok_or(ApiError::Unauthorized("MIGRATION_TOKEN_REQUIRED"))?;
    let hash = hex::encode(Sha256::digest(token.as_bytes()));

    let repo = PgAtMigrationRepository::new(state.db.clone());
    let row = repo
        .find_by_token_hash(&hash)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::Unauthorized("MIGRATION_TOKEN_INVALID"))?;
    if row.id != id {
        return Err(ApiError::Unauthorized("MIGRATION_TOKEN_INVALID"));
    }
    Ok(row)
}

#[derive(Deserialize)]
pub struct MigrationStartRequest {
    pub source_handle: String,
    pub source_password: String,
    /// seiran側で新規に名乗るユーザー名（移行元ハンドルとは独立）。
    pub new_username: String,
    /// seiran独自の新規パスワード。PDS Aのパスワードとは無関係で使い回さない。
    pub new_password: String,
    /// PDS Aがメール2FAを要求した場合の再試行時のみ指定する。
    pub auth_factor_token: Option<String>,
    /// seiran独自のアカウントメール（PDS Aのメールとは無関係）。
    /// `require_email_verification=false`のときのみ必須（`handlers::auth::register`と同じ形）。
    pub email: Option<String>,
}

#[derive(Serialize)]
pub struct MigrationStartResponse {
    pub request_id: i64,
    /// 以降の操作（`X-Migration-Token`ヘッダ）で使う生トークン。この応答一回きりでしか返らない。
    pub request_token: String,
    /// `awaiting_source_2fa` または `fetching_repo`。
    pub status: &'static str,
}

/// 256bit相当のランダムトークンを生成する（UUIDv4 2本連結、`getrandom`由来で暗号学的に安全）。
/// `rand`/`argon2`をseiran-apiクレートの新規依存に追加せずに済ませるため、
/// 既存依存の`uuid`（v4機能）を流用する。
fn generate_request_token() -> (String, String) {
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let hash = hex::encode(Sha256::digest(token.as_bytes()));
    (token, hash)
}

pub async fn start(
    State(state): State<AppState>,
    Json(req): Json<MigrationStartRequest>,
) -> Result<Json<MigrationStartResponse>, ApiError> {
    if req.source_handle.trim().is_empty() || req.source_password.is_empty() {
        return Err(ApiError::BadRequest("INVALID_INPUT".into()));
    }
    if req.new_password.len() < 8 {
        return Err(ApiError::BadRequest("INVALID_INPUT".into()));
    }
    if !seiran_common::is_valid_local_username(&req.new_username) {
        return Err(ApiError::BadRequest("USERNAME_INVALID_FORMAT".into()));
    }
    if seiran_common::is_reserved_username(&req.new_username) {
        return Err(ApiError::BadRequest("USERNAME_RESERVED".into()));
    }
    let username_exists = state
        .actors
        .find_including_withdrawn_by_username_domain(&req.new_username, &state.local_domain)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if username_exists.is_some() {
        return Err(ApiError::Conflict("USERNAME_TAKEN"));
    }

    // メールアドレス解決（`handlers::auth::register`と同じロジック）:
    // - `require_email_verification=false`ならこの時点で`email`必須、そのまま確定
    // - `true`なら`None`のままで進み、`awaiting_seiran_email`状態で
    //   `confirm_seiran_email`エンドポイントが後から確定させる
    let require_ev = state
        .site_settings
        .get("require_email_verification")
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map(|v| v == "true")
        .unwrap_or(false);
    let resolved_email: Option<String> = if require_ev {
        None
    } else {
        let email = req
            .email
            .as_deref()
            .filter(|e| !e.is_empty() && e.contains('@'))
            .ok_or_else(|| ApiError::BadRequest("INVALID_INPUT".into()))?
            .trim()
            .to_lowercase();
        let exists = state
            .users
            .email_exists(&email)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        if exists {
            return Err(ApiError::Conflict("EMAIL_ALREADY_REGISTERED"));
        }
        Some(email)
    };

    let (source_did, resolved) =
        seiran_common::atp::migration_client::resolve_source_pds(&req.source_handle, &state.ap_client.http)
            .await
            .map_err(|e| {
                tracing::info!("[migration:start] ハンドル解決失敗: {}", e);
                ApiError::BadRequest("SOURCE_HANDLE_UNRESOLVABLE".into())
            })?;

    // 転入元DIDが既に「ローカルアカウント」として使われていないか早期に弾く
    // （`actors.at_did UNIQUE`制約が最終的な防波堤だが、外部呼び出し前に弾ける方が親切）。
    // `actor_type <> 'local'`（bsky/fedi等のリモートキャッシュ行）は対象外——firehose購読や
    // プロフィール参照で既にDBに存在しているのは正常な状態であり、転入をブロックすべきではない。
    let did_in_use = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM actors WHERE at_did = $1 AND actor_type = 'local'",
    )
    .bind(&source_did)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    if did_in_use.is_some() {
        return Err(ApiError::Conflict("DID_ALREADY_REGISTERED"));
    }

    let session = match seiran_common::atp::migration_client::create_session_with_2fa(
        &resolved,
        &req.source_handle,
        &req.source_password,
        req.auth_factor_token.as_deref(),
    )
    .await
    {
        Ok(session) => session,
        Err(seiran_common::atp::migration_client::MigrationClientError::AuthFactorTokenRequired) => {
            // まだアカウント（DB行）を作らず、フロントに「PDS A宛メールのコードを入力して
            // auth_factor_token付きで再試行してください」と伝える。request_idはまだ無い。
            return Err(ApiError::BadRequest("AUTH_FACTOR_TOKEN_REQUIRED".into()));
        }
        Err(e) => {
            tracing::info!("[migration:start] 移行元PDS認証失敗: {}", e);
            return Err(ApiError::BadRequest("SOURCE_AUTH_FAILED".into()));
        }
    };

    let password_hash = LocalAuthProvider::hash_password(&req.new_password).map_err(|e| {
        tracing::error!("[migration:start] パスワードハッシュ失敗: {}", e);
        ApiError::Internal("パスワード処理エラー".to_string())
    })?;

    let request_id = seiran_common::generate_snowflake_id(chrono::Utc::now());
    let (request_token, request_token_hash) = generate_request_token();
    let now = chrono::Utc::now();

    let repo = PgAtMigrationRepository::new(state.db.clone());
    repo.create_request(
        request_id,
        &request_token_hash,
        &req.source_handle,
        &resolved.url,
        &source_did,
        &session.access_jwt,
        &session.refresh_jwt,
        &req.new_username,
        &password_hash,
        resolved_email.as_deref(),
        now,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    state.enqueue_migration_fetch_repo(request_id).await;

    Ok(Json(MigrationStartResponse {
        request_id,
        request_token,
        status: "fetching_repo",
    }))
}

#[derive(Deserialize)]
pub struct ConfirmSeiranEmailRequest {
    /// `POST /api/auth/verify-token` で得た`registration_token`（`email_verifications.token`）。
    pub registration_token: String,
}

#[derive(Serialize)]
pub struct MigrationStatusStub {
    pub status: &'static str,
}

/// `require_email_verification=true`のときのみ通る経路。seiran独自の確認メールに
/// 埋め込まれたリンクからユーザーが辿り着いた`registration_token`を消費し、
/// メールアドレスを確定させて`requesting_plc_signature`へ進める。
pub async fn confirm_seiran_email(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(req): Json<ConfirmSeiranEmailRequest>,
) -> Result<Json<MigrationStatusStub>, ApiError> {
    let migration_req = authorize_migration_request(&state, id, &headers).await?;
    if migration_req.status != "awaiting_seiran_email" {
        return Err(ApiError::BadRequest("INVALID_STATE".into()));
    }

    let token: uuid::Uuid = req
        .registration_token
        .trim()
        .parse()
        .map_err(|_| ApiError::BadRequest("REGISTRATION_TOKEN_INVALID".into()))?;
    let email = state
        .email_verifications
        .consume(token)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::BadRequest("REGISTRATION_TOKEN_INVALID".into()))?;

    let repo = PgAtMigrationRepository::new(state.db.clone());
    repo.set_email_and_status(id, &email, "requesting_plc_signature", chrono::Utc::now())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    state.enqueue_migration_request_plc_signature(id).await;

    Ok(Json(MigrationStatusStub {
        status: "requesting_plc_signature",
    }))
}

#[derive(Deserialize)]
pub struct SubmitPlcTokenRequest {
    /// PDS A登録メールに届いたコード（`requestPlcOperationSignature`発行分）。
    pub token: String,
}

/// ★不可逆境界。`signPlcOperation` → `submitPlcOperation` を実行し、成功したら
/// アカウント（`users`/`actors`）をDB確定する。`plc_genesis::register_plc_did`と同様、
/// 「外部への副作用のある呼び出しを完了させてからDB書き込みを行う」原則に従う。
///
/// **再入可能**: `submitPlcOperation`成功直後に`mark_plc_submitted`でDB記録するため、
/// その後のアカウント作成（`users`/`actors`）が失敗しても、`plc_submitted_at`が
/// 設定済み・`status='submitting_plc'`のリクエストに対して同エンドポイントを再度叩けば、
/// PLC提出をやり直さず（＝使用済みtokenを再送しない）保存済みの署名鍵でアカウント作成
/// だけをやり直す（実機で発見: `actors` INSERTがUNIQUE制約違反で失敗するケースがあり、
/// この再入可能性が無いと`awaiting_plc_token`のまま停滞し、既に消費済みのtokenで
/// ユーザーが再試行してしまっていた）。
///
/// `signPlcOperation`のtokenはPDS A側で15分・ワンタイムのため、`awaiting_plc_token`
/// からの一発勝負。失敗時はステータスを据え置き、ユーザーは`requesting_plc_signature`
/// から新しいメールトークンを取り直す。
pub async fn submit_plc_token(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(req): Json<SubmitPlcTokenRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let migration_req = authorize_migration_request(&state, id, &headers).await?;
    let email = migration_req
        .email
        .clone()
        .ok_or_else(|| ApiError::Internal("メールアドレス未確定".to_string()))?;

    let new_signing_key_pem = if let Some(existing_key) = migration_req.new_signing_key_pem.clone() {
        // 再入: PLCは既に提出済み。tokenは使わずアカウント作成のみやり直す。
        if migration_req.status != "submitting_plc" {
            return Err(ApiError::BadRequest("INVALID_STATE".into()));
        }
        tracing::info!(
            "[migration:submit-plc-token] request_id={} は再入（PLC提出済み、アカウント作成のみ再試行）",
            id
        );
        existing_key
    } else {
        if migration_req.status != "awaiting_plc_token" {
            return Err(ApiError::BadRequest("INVALID_STATE".into()));
        }

        let session = seiran_common::atp::migration_client::AtpSession {
            did: migration_req.source_did.clone(),
            handle: migration_req.source_handle.clone(),
            access_jwt: migration_req.source_access_jwt.clone().unwrap_or_default(),
            refresh_jwt: migration_req.source_refresh_jwt.clone().unwrap_or_default(),
        };
        // `start`時点で確定したPDS Aのエンドポイント文字列をそのまま使う（DID文書からの
        // 再導出ではない——`resolve_stored_endpoint`のドキュメントコメント参照。実機で発見:
        // DID文書から再導出すると、PLC操作が既に成功した後の再試行時に移行先(seiran自身)を
        // 指してしまう）。
        let resolved = seiran_common::atp::did_resolve::resolve_stored_endpoint(
            &migration_req.source_pds_endpoint,
        )
        .await
        .map_err(|e| {
                tracing::warn!("[migration:submit-plc-token] PDSエンドポイント検証失敗: {}", e);
                ApiError::BadGateway("SOURCE_PDS_UNREACHABLE".into())
            })?;

        // 現在のDIDドキュメントからrotationKeysを取得し、変更せずそのまま引き継ぐ
        // （seiranは鍵の管理権限を奪わない——実装方針の議論参照）。公開DIDドキュメント
        // （plc.directory/{did}）にはrotationKeysが含まれないため`/data`エンドポイントを使う。
        let plc_data_url = format!(
            "{}/{}/data",
            seiran_common::atp::plc::plc_directory_base_url(),
            migration_req.source_did
        );
        let current_data: serde_json::Value = state
            .http_client
            .get(&plc_data_url)
            .send()
            .await
            .map_err(|e| {
                tracing::warn!("[migration:submit-plc-token] plc.directory取得失敗: {}", e);
                ApiError::BadGateway("PLC_DIRECTORY_UNREACHABLE".into())
            })?
            .json()
            .await
            .map_err(|e| {
                tracing::warn!("[migration:submit-plc-token] plc.directoryレスポンス解析失敗: {}", e);
                ApiError::BadGateway("PLC_DIRECTORY_UNREACHABLE".into())
            })?;
        let rotation_keys: Vec<String> = current_data
            .get("rotationKeys")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if rotation_keys.is_empty() {
            tracing::error!(
                "[migration:submit-plc-token] rotationKeys取得失敗 (request_id={}, did={})",
                id, migration_req.source_did
            );
            return Err(ApiError::Internal("ROTATION_KEYS_UNAVAILABLE".to_string()));
        }

        let (_new_signing_key, new_signing_key_pem) =
            seiran_common::atp::plc::generate_new_signing_key().map_err(|e| {
                tracing::error!("[migration:submit-plc-token] 鍵生成失敗: {}", e);
                ApiError::Internal("鍵生成エラー".to_string())
            })?;
        let new_did_key = seiran_common::atp::plc::p256_to_did_key(
            seiran_common::atp::plc::signing_key_from_pem(&new_signing_key_pem)
                .map_err(|e| ApiError::Internal(e.to_string()))?
                .verifying_key(),
        );

        let atp_username = seiran_common::username::to_atp_username(&migration_req.new_username);
        let handle = format!("{}.{}", atp_username, state.local_domain);
        let pds_endpoint = format!("https://{}", state.local_domain);

        // Cloudflare TXTセット（ベストエフォート、plc_genesis::register_plc_didと同じ扱い）。
        // Cloudflare未設定の環境（このdev環境含む）では自然にNoneとなり、ハンドル検証は
        // `/.well-known/atproto-did`（seiranが常時実装済み）に一本化される。
        let cf_record_id = if let Some(cf) = &state.cloudflare {
            match cf.set_atproto_txt(&handle, &migration_req.source_did).await {
                Ok(record_id) => {
                    tracing::info!(
                        "[migration:submit-plc-token] Cloudflare TXT セット完了: _atproto.{}",
                        handle
                    );
                    Some(record_id)
                }
                Err(e) => {
                    tracing::error!(
                        "[migration:submit-plc-token] Cloudflare TXT セット失敗（続行）: {}",
                        e
                    );
                    None
                }
            }
        } else {
            None
        };
        let _ = cf_record_id;

        let desired = seiran_common::atp::migration_client::DesiredDidCredentials {
            rotation_keys,
            also_known_as: vec![format!("at://{}", handle)],
            verification_methods: serde_json::json!({ "atproto": new_did_key }),
            services: serde_json::json!({
                "atproto_pds": { "type": "AtprotoPersonalDataServer", "endpoint": pds_endpoint }
            }),
        };

        let operation = seiran_common::atp::migration_client::sign_plc_operation(
            &resolved, &session, &req.token, &desired,
        )
        .await
        .map_err(|e| {
            tracing::warn!("[migration:submit-plc-token] signPlcOperation失敗 (request_id={}): {}", id, e);
            ApiError::BadGateway("PLC_SIGN_FAILED".into())
        })?;

        // ─────────────────────────────────────────────────────────────
        // ★不可逆境界: ここから先、DIDのservice endpointはseiranを指すようになる。
        // ─────────────────────────────────────────────────────────────
        seiran_common::atp::migration_client::submit_plc_operation(
            &migration_req.source_did,
            &operation,
            &state.http_client,
        )
        .await
        .map_err(|e| {
            tracing::error!(
                "[migration:submit-plc-token] submitPlcOperation失敗 (request_id={}): {}",
                id,
                e
            );
            ApiError::Internal("PLC_SUBMIT_FAILED".into())
        })?;

        // 不可逆操作は完了した。以降のアカウント作成が失敗してもこの事実を必ず残す
        // （このUPDATE自体が失敗した場合は仕方なくエラーを返すが、実際のPLC状態と
        // DBの食い違いが起きるのはこの一箇所だけに限定される）。
        let repo = PgAtMigrationRepository::new(state.db.clone());
        repo.mark_plc_submitted(id, &new_signing_key_pem, chrono::Utc::now())
            .await
            .map_err(|e| {
                tracing::error!(
                    "[migration:submit-plc-token] mark_plc_submitted失敗（PLCは提出済み！） (request_id={}): {}",
                    id, e
                );
                ApiError::Internal(format!(
                    "PLC提出は成功しましたが記録に失敗しました。手動確認が必要です: {e}"
                ))
            })?;

        new_signing_key_pem
    };

    // ここから先はresume経路と共通。副作用のある外部呼び出しは完了済みなので、
    // ロールバックせずログを残しつつ可能な限り完了させる（`plc_genesis`/`register`と同じ原則）。

    // 転入元DIDが既にseiranの`actors`にリモートキャッシュ行として存在することがある
    // （firehose購読やプロフィール参照で自然に発生、`start`時点の重複チェックは
    // `actor_type='local'`のみ対象にしているため素通りする——実機で発見）。
    // 存在すればローカル用に変換（UPDATE）、無ければ新規作成（INSERT）する。
    let existing_actor_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM actors WHERE at_did = $1")
            .bind(&migration_req.source_did)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

    let existing_user_id: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(&email)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let user_id = if let Some(uid) = existing_user_id {
        uid
    } else {
        state
            .users
            .insert(&email, &migration_req.password_hash, "user")
            .await
            .map_err(|e| {
                tracing::error!("[migration:submit-plc-token] users INSERT 失敗: {}", e);
                ApiError::Internal("ユーザー作成エラー".to_string())
            })?
    };

    let actor_id = if let Some(existing_id) = existing_actor_id {
        let ap_uri = format!("https://{}/users/{}", state.local_domain, migration_req.new_username);
        sqlx::query(
            "UPDATE actors SET actor_type = 'local', user_id = $1, username = $2, domain = $3,
                 ap_uri = $4, at_signing_key_pem = $5, updated_at = NOW()
             WHERE id = $6",
        )
        .bind(user_id)
        .bind(&migration_req.new_username)
        .bind(state.local_domain.as_str())
        .bind(&ap_uri)
        .bind(&new_signing_key_pem)
        .bind(existing_id)
        .execute(&state.db)
        .await
        .map_err(|e| {
            tracing::error!(
                "[migration:submit-plc-token] actors UPDATE（リモート→ローカル変換）失敗: {}",
                e
            );
            ApiError::Internal("アクター変換エラー".to_string())
        })?;
        existing_id
    } else {
        let new_id = seiran_common::generate_snowflake_id(chrono::Utc::now());
        state
            .actors
            .insert_local(
                new_id,
                user_id,
                &migration_req.new_username,
                &state.local_domain,
                Some(&migration_req.source_did),
                Some(&new_signing_key_pem),
                None,
            )
            .await
            .map_err(|e| {
                tracing::error!("[migration:submit-plc-token] actors INSERT 失敗: {}", e);
                ApiError::Internal("アクター作成エラー".to_string())
            })?;
        new_id
    };

    let repo = PgAtMigrationRepository::new(state.db.clone());
    if let Err(e) = repo
        .confirm_account_created(id, actor_id, user_id, chrono::Utc::now())
        .await
    {
        // at_migration_requests側の記録に失敗しても、users/actorsは既に確定済みなので
        // アカウント作成自体は成立している（ログのみ、レスポンスは正常に返す）。
        tracing::error!(
            "[migration:submit-plc-token] at_migration_requests確定記録失敗 (request_id={}): {}",
            id,
            e
        );
    }

    state.enqueue_migration_import_process(id).await;

    let (token, _jti) = state.local_auth.generate_token(user_id, &email).map_err(|e| {
        tracing::error!("[migration:submit-plc-token] JWT 生成失敗: {}", e);
        ApiError::Internal("トークン生成エラー".to_string())
    })?;

    Ok(Json(AuthResponse {
        token: token.clone(),
        user: UserInfo {
            id: user_id,
            username: migration_req.new_username,
            email,
            role: "user".to_string(),
            actor_id,
            avatar_url: Some(seiran_common::avatar::fallback_avatar_url(
                &state.local_domain,
                actor_id,
            )),
            language_preference: None,
            token,
            is_suspended: false,
            // このレスポンスを返す時点でステータスは必ず`importing_data`
            // （直前の`confirm_account_created`が確定させる値）。
            migration_status: Some("importing_data".to_string()),
        },
    }))
}

#[derive(Serialize)]
pub struct MigrationStatusResponse {
    pub request_id: i64,
    pub status: String,
    pub last_error: Option<String>,
    /// フロントが表示すべき入力欄の種類。`None`なら入力欄は不要。
    pub needs_input: Option<&'static str>,
    /// 「リトライ」ボタンを表示してよいか（バックグラウンドジョブが動くステップのみ）。
    pub retryable: bool,
    /// `plc_submitted_at`が未設定（不可逆境界の前）なら真。「別DIDで再開／新規DID切替」を
    /// 提示してよいかの判定に使う。
    pub can_abandon: bool,
}

fn needs_input_for(status: &str) -> Option<&'static str> {
    match status {
        "awaiting_seiran_email" => Some("seiran_email_token"),
        "awaiting_plc_token" => Some("plc_token"),
        _ => None,
    }
}

/// ジョブが裏で動くステータス（`/retry`で再enqueueできる対象）。
fn is_job_driven_status(status: &str) -> bool {
    matches!(
        status,
        "fetching_repo" | "requesting_plc_signature" | "importing_data" | "deactivating_source"
    )
}

pub async fn get_status(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Json<MigrationStatusResponse>, ApiError> {
    let migration_req = authorize_migration_request(&state, id, &headers).await?;
    Ok(Json(MigrationStatusResponse {
        request_id: id,
        status: migration_req.status.clone(),
        last_error: migration_req.last_error,
        needs_input: needs_input_for(&migration_req.status),
        retryable: is_job_driven_status(&migration_req.status),
        can_abandon: migration_req.plc_submitted_at.is_none()
            && migration_req.status != "completed"
            && migration_req.status != "abandoned",
    }))
}

/// 現在のステータスに対応するジョブを再度積む。ジョブが存在しないステータス
/// （`awaiting_*`・`submitting_plc`・`completed`・`abandoned`等）には使えない
/// ——`awaiting_plc_token`は`submit-plc-token`を、`awaiting_seiran_email`は
/// `confirm-seiran-email`を、`submitting_plc`（PLC提出済みの再入）は
/// `submit-plc-token`（token省略可）をそれぞれ呼び直すこと。
pub async fn retry(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Json<MigrationStatusStub>, ApiError> {
    let migration_req = authorize_migration_request(&state, id, &headers).await?;
    match migration_req.status.as_str() {
        "fetching_repo" => {
            state.enqueue_migration_fetch_repo(id).await;
            Ok(Json(MigrationStatusStub { status: "fetching_repo" }))
        }
        "requesting_plc_signature" => {
            state.enqueue_migration_request_plc_signature(id).await;
            Ok(Json(MigrationStatusStub {
                status: "requesting_plc_signature",
            }))
        }
        "importing_data" => {
            state.enqueue_migration_import_process(id).await;
            Ok(Json(MigrationStatusStub { status: "importing_data" }))
        }
        "deactivating_source" => {
            state.enqueue_migration_deactivate_source(id).await;
            Ok(Json(MigrationStatusStub {
                status: "deactivating_source",
            }))
        }
        _ => Err(ApiError::BadRequest("NOT_RETRYABLE".into())),
    }
}

/// `plc_submitted_at`が未設定（不可逆境界の前）のリクエストのみ打ち切れる。
/// フロント側はこの後「別DIDで再開」（`/api/migration/start`を新しい`source_handle`で
/// 再度叩く）か「新規DID立ち上げ」（通常の`/api/auth/register`）へ遷移させる。
pub async fn abandon(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Json<MigrationStatusStub>, ApiError> {
    let migration_req = authorize_migration_request(&state, id, &headers).await?;
    if migration_req.plc_submitted_at.is_some() {
        return Err(ApiError::BadRequest("CANNOT_ABANDON_AFTER_PLC_SUBMIT".into()));
    }
    let repo = PgAtMigrationRepository::new(state.db.clone());
    repo.set_status(id, "abandoned", chrono::Utc::now())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(MigrationStatusStub { status: "abandoned" }))
}

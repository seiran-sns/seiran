//! 既存DID転入フロー（`docs/account_migration.md`）のエントリーポイント。
//!
//! `submitting_plc`成功までアカウント（`users`/`actors`）が存在しないため、通常のJWT認証は
//! 使えない。`password_reset`と同様、ランダムトークンをSHA-256ハッシュ化してDB保存し、
//! レスポンス一回きりで生値を返す方式（`X-Migration-Token`ヘッダで以降の操作を認可する、
//! 本ファイルでは`start`のみ実装——ステータス確認・トークン入力等は後続フェーズで追加）。

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use seiran_common::repository::{
    AtMigrationRepository, AtMigrationRequestRow, PgAtMigrationRepository,
};
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
    /// PDS Aが`createSession`でメールアドレスを返さなかった場合のみのフォールバック
    /// （通常はPDS A側の登録済みメールをそのまま使うため空でよい）。
    pub email: Option<String>,
    /// 利用規約同意チェックボックス。`handlers::auth::resolve_tos_agreement`参照
    /// （`register`と同じ検証・同じ`site_settings`キーを共有する）。
    pub agree_tos: Option<bool>,
}

#[derive(Serialize)]
pub struct MigrationStartResponse {
    /// snowflake IDはJSの53bit整数精度を超えるため文字列で返す（`NoteUserInfo.id`等と同じ理由）。
    pub request_id: String,
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
    crate::handlers::auth::ensure_registration_open(&state).await?;
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

    let (source_did, resolved) = seiran_common::atp::migration_client::resolve_source_pds(
        &req.source_handle,
        &state.ap_client.http,
    )
    .await
    .map_err(|e| {
        tracing::info!("[migration:start] ハンドル解決失敗: {}", e);
        ApiError::BadRequest("SOURCE_HANDLE_UNRESOLVABLE".into())
    })?;

    // 転入元DIDが既に「ローカルアカウント」として使われていないか早期に弾く
    // （`actors.at_did UNIQUE`制約が最終的な防波堤だが、外部呼び出し前に弾ける方が親切）。
    // `actor_type <> 'local'`（bsky/fedi等のリモートキャッシュ行）は対象外——firehose購読や
    // プロフィール参照で既にDBに存在しているのは正常な状態であり、転入をブロックすべきではない。
    let did_in_use = state
        .actors
        .find_by_did(&source_did)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .is_some_and(|a| a.actor_type == "local");
    if did_in_use {
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
        Err(
            seiran_common::atp::migration_client::MigrationClientError::AuthFactorTokenRequired,
        ) => {
            // まだアカウント（DB行）を作らず、フロントに「PDS A宛メールのコードを入力して
            // auth_factor_token付きで再試行してください」と伝える。request_idはまだ無い。
            return Err(ApiError::BadRequest("AUTH_FACTOR_TOKEN_REQUIRED".into()));
        }
        Err(e) => {
            tracing::info!("[migration:start] 移行元PDS認証失敗: {}", e);
            return Err(ApiError::BadRequest("SOURCE_AUTH_FAILED".into()));
        }
    };

    // メールアドレス解決: 転入元PDSは`createSession`のパスワード認証を既に通っているため、
    // 独自のメール実在確認は不要——PDS Aに登録済みのメールをそのまま信頼して使う。
    // PDS Aがメールを返さない場合（Blueskyのapp password認証ではメイン
    // パスワード認証と異なり`email`/`emailConfirmed`が返らない）は`req.email`に
    // フォールバックする。それも無ければフロントに専用エラーを返し、メール入力欄付きで
    // 再試行させる（`AUTH_FACTOR_TOKEN_REQUIRED`と同じ「エラーで欄を追加して再送」パターン）。
    let email = match session.email.as_deref() {
        Some(e) if !e.is_empty() => e.trim().to_lowercase(),
        _ => match req
            .email
            .as_deref()
            .filter(|e| !e.is_empty() && e.contains('@'))
        {
            Some(e) => e.trim().to_lowercase(),
            None => {
                tracing::info!(
                    "[migration:start] PDS Aがメールを返さずreq.emailも未指定のため中断（handle={}）",
                    req.source_handle
                );
                return Err(ApiError::BadRequest("SOURCE_EMAIL_REQUIRED".into()));
            }
        },
    };
    let email_exists = state
        .users
        .email_exists(&email)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if email_exists {
        return Err(ApiError::Conflict("EMAIL_ALREADY_REGISTERED"));
    }

    let agreed_tos_text =
        crate::handlers::auth::resolve_tos_agreement(&state, req.agree_tos.unwrap_or(false))
            .await?;

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
        &seiran_common::repository::NewMigrationRequest {
            request_token_hash: &request_token_hash,
            source_handle: &req.source_handle,
            source_pds_endpoint: &resolved.url,
            source_did: &source_did,
            source_access_jwt: &session.access_jwt,
            source_refresh_jwt: &session.refresh_jwt,
            new_username: &req.new_username,
            password_hash: &password_hash,
            email: Some(&email),
            agreed_tos_text: agreed_tos_text.as_deref(),
        },
        now,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    state.enqueue_migration_fetch_repo(request_id).await;

    Ok(Json(MigrationStartResponse {
        request_id: request_id.to_string(),
        request_token,
        status: "fetching_repo",
    }))
}

#[derive(Serialize)]
pub struct MigrationStatusStub {
    pub status: &'static str,
}

#[derive(Deserialize)]
pub struct SubmitPlcTokenRequest {
    /// PDS A登録メールに届いたコード（`requestPlcOperationSignature`発行分）。
    pub token: String,
}

/// `submit-plc-token` の応答。DNS TXT の公開を確かめてから PLC 送信へ進むため、その場で
/// アカウントまで確定できたときだけ JWT を返し、待ちが残ったら状態だけを返す（フロントは
/// `/status` をポーリングし、アカウントができたら `/session` で JWT を受け取る）。
#[derive(Serialize)]
#[serde(untagged)]
pub enum SubmitPlcTokenResponse {
    Auth(Box<AuthResponse>),
    Pending { status: &'static str },
}

/// PDS A に PLC 操作へ署名させ（`signPlcOperation`、メールのワンタイムトークンを消費する）、
/// 署名済みの操作を保存して「DNS TXT 登録完了待ち」（`awaiting_dns_txt`）にする。PLC 送信・
/// アカウント確定は `seiran_common::did_provision::migration` が TXT の公開を確かめてから行う。
///
/// **再入可能**: PLC 送信後にアカウント作成だけが失敗した（`submitting_plc`・鍵記録済み）
/// リクエストに対して同エンドポイントを再度叩けば、PLC 提出をやり直さず（＝使用済みtokenを
/// 再送しない）保存済みの署名鍵でアカウント作成だけをやり直す。
///
/// `signPlcOperation`のtokenはPDS A側で15分・ワンタイムのため、`awaiting_plc_token`
/// からの一発勝負。署名に失敗したらステータスを据え置き、ユーザーは`requesting_plc_signature`
/// から新しいメールトークンを取り直す。
pub async fn submit_plc_token(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(req): Json<SubmitPlcTokenRequest>,
) -> Result<Json<SubmitPlcTokenResponse>, ApiError> {
    let migration_req = authorize_migration_request(&state, id, &headers).await?;
    if let (Some(signing_key_pem), Some(rotation_key_pem)) = (
        migration_req.new_signing_key_pem.as_deref(),
        migration_req.new_rotation_key_pem.as_deref(),
    ) {
        // 再入: PLCは既に提出済み。tokenは使わずアカウント作成のみやり直す。
        if migration_req.status != "submitting_plc" {
            return Err(ApiError::BadRequest("INVALID_STATE".into()));
        }
        tracing::info!(
            "[migration:submit-plc-token] request_id={} は再入（PLC提出済み、アカウント作成のみ再試行）",
            id
        );
        let (user_id, actor_id) =
            materialize_account(&state, &migration_req, signing_key_pem, rotation_key_pem).await?;
        let repo = PgAtMigrationRepository::new(state.db.clone());
        if let Err(e) = repo
            .confirm_account_created(id, actor_id, user_id, chrono::Utc::now())
            .await
        {
            // users/actors は確定済みなのでアカウント作成自体は成立している（ログのみ）。
            tracing::error!(
                "[migration:submit-plc-token] at_migration_requests確定記録失敗 (request_id={}): {}",
                id,
                e
            );
        }
        state.enqueue_migration_import_process(id).await;
        return Ok(Json(SubmitPlcTokenResponse::Auth(Box::new(
            account_auth_response(&state, id).await?,
        ))));
    }

    if migration_req.status != "awaiting_plc_token" {
        return Err(ApiError::BadRequest("INVALID_STATE".into()));
    }
    // 不可逆なPLC提出の前に弾く（提出後に弾くとDIDだけ移ってアカウントが作られない）。
    crate::handlers::auth::ensure_registration_open(&state).await?;
    stage_plc_operation(&state, &migration_req, &req.token).await?;

    // 1段進め（Cloudflare 未設定ならここでアカウント確定まで終わる）、残りはバックグラウンドで進める。
    if state.did_provisioner.advance_migration(id).await {
        state.did_provisioner.kick_migration(id);
        return Ok(Json(SubmitPlcTokenResponse::Pending {
            status: "awaiting_dns_txt",
        }));
    }
    Ok(Json(SubmitPlcTokenResponse::Auth(Box::new(
        account_auth_response(&state, id).await?,
    ))))
}

/// 転入で作られたアカウントの JWT を発行する。`awaiting_dns_txt` の待ちが明けてアカウントが
/// できたとき、ログイン前の転入画面が `X-Migration-Token` で JWT を受け取るのに使う
/// （`submit-plc-token` がその場で確定できた場合に返すものと同じ）。
pub async fn session(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Json<AuthResponse>, ApiError> {
    authorize_migration_request(&state, id, &headers).await?;
    Ok(Json(account_auth_response(&state, id).await?))
}

/// 転入リクエストに紐づくアカウントの JWT とプロフィール。アカウントが未確定なら、
/// 失敗理由があればそれを、無ければ `ACCOUNT_NOT_READY` を返す。
async fn account_auth_response(state: &AppState, id: i64) -> Result<AuthResponse, ApiError> {
    let migration_req = PgAtMigrationRepository::new(state.db.clone())
        .get(id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?;
    let (Some(user_id), Some(actor_id), Some(email)) = (
        migration_req.user_id,
        migration_req.actor_id,
        migration_req.email.clone(),
    ) else {
        if migration_req.status == "failed" {
            return Err(ApiError::BadGateway("PLC_SUBMIT_FAILED".into()));
        }
        return Err(ApiError::Conflict("ACCOUNT_NOT_READY"));
    };
    let (token, _jti) = state
        .local_auth
        .generate_token(user_id, &email)
        .map_err(|e| {
            tracing::error!("[migration] JWT 生成失敗: {}", e);
            ApiError::Internal("トークン生成エラー".to_string())
        })?;
    let migration_status = (migration_req.status != "completed").then_some(migration_req.status);
    Ok(AuthResponse {
        token: token.clone(),
        user: UserInfo {
            id: user_id,
            username: migration_req.new_username,
            email,
            role: "user".to_string(),
            actor_id: actor_id.to_string(),
            avatar_url: Some(seiran_common::avatar::fallback_avatar_url(
                &state.local_domain,
                actor_id,
            )),
            language_preference: None,
            token,
            is_suspended: false,
            migration_status,
            did_moved_out: false, // 転入直後はDID転出済みであり得ない
        },
    })
}

/// 新しい鍵ペア（PEM, did:key）を生成する。
fn generate_did_key(label: &str) -> Result<(String, String), ApiError> {
    let (_key, pem) = seiran_common::atp::plc::generate_new_signing_key().map_err(|e| {
        tracing::error!("[migration:submit-plc-token] {}生成失敗: {}", label, e);
        ApiError::Internal("鍵生成エラー".to_string())
    })?;
    let did_key = seiran_common::atp::plc::p256_to_did_key(
        seiran_common::atp::plc::signing_key_from_pem(&pem)
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .verifying_key(),
    );
    Ok((pem, did_key))
}

/// PDS A に、DID の鍵・ハンドル・PDS を seiran へ向け替える PLC 操作へ署名させ
/// （`signPlcOperation`）、署名済みの操作と新しい鍵を保存して `awaiting_dns_txt` にする。
async fn stage_plc_operation(
    state: &AppState,
    migration_req: &AtMigrationRequestRow,
    plc_token: &str,
) -> Result<(), ApiError> {
    let id = migration_req.id;
    let session = seiran_common::atp::migration_client::AtpSession {
        did: migration_req.source_did.clone(),
        handle: migration_req.source_handle.clone(),
        access_jwt: migration_req.source_access_jwt.clone().unwrap_or_default(),
        refresh_jwt: migration_req.source_refresh_jwt.clone().unwrap_or_default(),
        email: None,
        email_confirmed: false,
    };
    // `start`時点で確定したPDS Aのエンドポイント文字列をそのまま使う（DID文書からの
    // 再導出ではない——`resolve_stored_endpoint`のドキュメントコメント参照。
    // DID文書から再導出すると、PLC操作が既に成功した後の再試行時に移行先(seiran自身)を
    // 指してしまう）。
    let resolved = seiran_common::atp::did_resolve::resolve_stored_endpoint(
        &migration_req.source_pds_endpoint,
    )
    .await
    .map_err(|e| {
        tracing::warn!(
            "[migration:submit-plc-token] PDSエンドポイント検証失敗: {}",
            e
        );
        ApiError::BadGateway("SOURCE_PDS_UNREACHABLE".into())
    })?;

    // 転入完了時、seiranが新規発行する専用のローテーションキーのみをDIDの鍵とする
    // （転入元PDS運営者に恒久的な支配権を残さないため、転入元の鍵は引き継がない）。
    let (new_signing_key_pem, new_did_key) = generate_did_key("鍵")?;
    let (new_rotation_key_pem, new_rotation_did_key) = generate_did_key("ローテーション鍵")?;

    let atp_username = seiran_common::username::to_atp_username(&migration_req.new_username);
    let handle = format!("{}.{}", atp_username, state.local_domain);
    let pds_endpoint = format!("https://{}", state.local_domain);
    let desired = seiran_common::atp::migration_client::DesiredDidCredentials {
        rotation_keys: vec![new_rotation_did_key],
        also_known_as: vec![format!("at://{}", handle)],
        verification_methods: serde_json::json!({ "atproto": new_did_key }),
        services: serde_json::json!({
            "atproto_pds": { "type": "AtprotoPersonalDataServer", "endpoint": pds_endpoint }
        }),
    };

    let operation = seiran_common::atp::migration_client::sign_plc_operation(
        &resolved, &session, plc_token, &desired,
    )
    .await
    .map_err(|e| {
        tracing::warn!(
            "[migration:submit-plc-token] signPlcOperation失敗 (request_id={}): {}",
            id,
            e
        );
        ApiError::BadGateway("PLC_SIGN_FAILED".into())
    })?;

    let staged = seiran_common::repository::at_migration::stage_plc_operation(
        &state.db,
        id,
        &operation,
        &new_signing_key_pem,
        &new_rotation_key_pem,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !staged {
        return Err(ApiError::BadRequest("INVALID_STATE".into()));
    }
    Ok(())
}

/// 転入したアカウントの `users`/`actors` 行を確定し、（user_id, actor_id）を返す。
async fn materialize_account(
    state: &AppState,
    migration_req: &AtMigrationRequestRow,
    signing_key_pem: &str,
    rotation_key_pem: &str,
) -> Result<(i64, i64), ApiError> {
    let email = migration_req
        .email
        .as_deref()
        .ok_or_else(|| ApiError::Internal("メールアドレス未確定".to_string()))?;
    seiran_common::did_provision::migration::materialize_local_account(
        &state.db,
        &state.local_domain,
        migration_req,
        email,
        signing_key_pem,
        rotation_key_pem,
    )
    .await
    .map_err(|e| {
        tracing::error!("[migration:submit-plc-token] アカウント作成失敗: {}", e);
        ApiError::Internal("アカウント作成エラー".to_string())
    })
}

#[derive(Serialize)]
pub struct MigrationStatusResponse {
    /// snowflake IDはJSの53bit整数精度を超えるため文字列で返す（`NoteUserInfo.id`等と同じ理由）。
    pub request_id: String,
    pub status: String,
    pub last_error: Option<String>,
    /// フロントが表示すべき入力欄の種類。`None`なら入力欄は不要。
    pub needs_input: Option<&'static str>,
    /// 「リトライ」ボタンを表示してよいか（バックグラウンドジョブが動くステップのみ）。
    pub retryable: bool,
    /// `plc_submitted_at`が未設定（不可逆境界の前）なら真。「別DIDで再開／新規DID切替」を
    /// 提示してよいかの判定に使う。
    pub can_abandon: bool,
    /// `status == "importing_data"`のときのみ`Some`（取り込み済み件数/全体件数、レコード+blob合算）。
    pub import_done: Option<i64>,
    pub import_total: Option<i64>,
}

fn needs_input_for(status: &str) -> Option<&'static str> {
    match status {
        "awaiting_plc_token" => Some("plc_token"),
        _ => None,
    }
}

/// 裏で進むステータス（`/retry`で再enqueueできる対象。`awaiting_dns_txt`はジョブではなく
/// `did_provision`の定時処理が進め、`/retry`はその場で進め直す）。
fn is_job_driven_status(status: &str) -> bool {
    matches!(
        status,
        "fetching_repo"
            | "requesting_plc_signature"
            | "awaiting_dns_txt"
            | "importing_data"
            | "deactivating_source"
    )
}

pub async fn get_status(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Json<MigrationStatusResponse>, ApiError> {
    let mut migration_req = authorize_migration_request(&state, id, &headers).await?;
    // DNS TXT の公開待ちなら、ポーリングのついでに1段進める（定時処理の間隔を待たない）。
    if migration_req.status == "awaiting_dns_txt" {
        state.did_provisioner.advance_migration(id).await;
        migration_req = PgAtMigrationRepository::new(state.db.clone())
            .get(id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .ok_or(ApiError::NotFound("NOT_FOUND"))?;
    }

    let import_progress = if migration_req.status == "importing_data" {
        let repo = PgAtMigrationRepository::new(state.db.clone());
        let (done, total) = repo
            .import_progress(id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        Some((done, total))
    } else {
        None
    };

    Ok(Json(MigrationStatusResponse {
        request_id: id.to_string(),
        status: migration_req.status.clone(),
        last_error: migration_req.last_error,
        needs_input: needs_input_for(&migration_req.status),
        retryable: is_job_driven_status(&migration_req.status),
        can_abandon: migration_req.plc_submitted_at.is_none()
            && migration_req.status != "submitting_plc"
            && migration_req.status != "completed"
            && migration_req.status != "abandoned",
        import_done: import_progress.map(|(done, _)| done),
        import_total: import_progress.map(|(_, total)| total),
    }))
}

/// 現在のステータスに対応するジョブを再度積む。ジョブが存在しないステータス
/// （`awaiting_*`・`submitting_plc`・`completed`・`abandoned`等）には使えない
/// ——`awaiting_plc_token`は`submit-plc-token`を、`submitting_plc`（PLC提出済みの再入）は
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
            Ok(Json(MigrationStatusStub {
                status: "fetching_repo",
            }))
        }
        "requesting_plc_signature" => {
            state.enqueue_migration_request_plc_signature(id).await;
            Ok(Json(MigrationStatusStub {
                status: "requesting_plc_signature",
            }))
        }
        "awaiting_dns_txt" => {
            if state.did_provisioner.advance_migration(id).await {
                state.did_provisioner.kick_migration(id);
            }
            Ok(Json(MigrationStatusStub {
                status: "awaiting_dns_txt",
            }))
        }
        "importing_data" => {
            state.enqueue_migration_import_process(id).await;
            Ok(Json(MigrationStatusStub {
                status: "importing_data",
            }))
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
        return Err(ApiError::BadRequest(
            "CANNOT_ABANDON_AFTER_PLC_SUBMIT".into(),
        ));
    }
    // PLC 送信中（`submitting_plc`）は打ち切れない。状態が読んだ時点から変わっていたら
    // （`awaiting_dns_txt` から PLC 送信が始まった等）打ち切らない。
    if migration_req.status == "submitting_plc"
        || !seiran_common::repository::at_migration::transition_status(
            &state.db,
            id,
            &migration_req.status,
            "abandoned",
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        return Err(ApiError::Conflict("CANNOT_ABANDON_NOW"));
    }
    Ok(Json(MigrationStatusStub {
        status: "abandoned",
    }))
}

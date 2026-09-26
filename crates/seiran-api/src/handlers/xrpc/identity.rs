//! `com.atproto.identity.*` — 転出元API対応（seiranが転出元PDSとして振る舞う経路）。
//! `crates/seiran-common/src/atp/migration_client.rs`（seiranが転入先として他PDSへ
//! 呼ぶクライアント側）と対になる、サーバー側の実装。

use axum::{extract::State, http::HeaderMap, response::IntoResponse, Json};
use serde::Deserialize;
use sha2::Digest;

use seiran_common::atp::plc::{
    fetch_current_plc_doc_and_prev, p256_to_did_key, prepare_plc_rotation_update,
    signing_key_from_pem,
};

use super::{extract_bearer, service_did};
use crate::error::ApiError;
use crate::AppState;

const PLC_SIGNATURE_PURPOSE: &str = "plc_operation_signature";

/// `com.atproto.identity.getRecommendedDidCredentials` — 現在のDIDドキュメントの
/// 推奨フィールド（転出先クライアントが引き継ぐべき値）を返す、読み取りのみ。
pub async fn xrpc_get_recommended_did_credentials(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let Some(token) = extract_bearer(&headers) else {
        return ApiError::Unauthorized("Authorization ヘッダーが必要です").into_response();
    };
    let verified = match state
        .local_auth
        .verify_atp_access_token(token, &service_did(&state))
    {
        Ok(v) => v,
        Err(_) => return ApiError::Unauthorized("トークンが無効です").into_response(),
    };
    let actor = match state.actors.find_by_did(&verified.did).await {
        Ok(Some(a)) => a,
        _ => return ApiError::Unauthorized("アクターが見つかりません").into_response(),
    };

    let rotation_keys = current_rotation_key_dids(&state, &actor);

    let Some(signing_key_pem) = actor.at_signing_key_pem.as_deref() else {
        return ApiError::Internal("署名鍵が未設定です".to_string()).into_response();
    };
    let signing_did_key = match signing_key_from_pem(signing_key_pem) {
        Ok(k) => p256_to_did_key(k.verifying_key()),
        Err(e) => return ApiError::Internal(format!("署名鍵パース失敗: {e}")).into_response(),
    };

    let handle = format!("{}.{}", actor.username, state.local_domain);
    let pds_endpoint = format!("https://{}", state.local_domain);

    Json(serde_json::json!({
        "rotationKeys": rotation_keys,
        "alsoKnownAs": [format!("at://{}", handle)],
        "verificationMethods": { "atproto": signing_did_key },
        "services": {
            "atproto_pds": { "type": "AtprotoPersonalDataServer", "endpoint": pds_endpoint }
        },
    }))
    .into_response()
}

/// アカウント単位ローテーションキー（`at_rotation_key_pem`）があれば
/// `[アカウント鍵, サーバー共有鍵]`、無ければ`[サーバー共有鍵]`のみを返す
/// （後者は理論上のフォールバックで、現状のアクティブなローカルアカウントには
/// 発生しない——Phase Aバックフィル完了済み）。
fn current_rotation_key_dids(
    state: &AppState,
    actor: &seiran_common::repository::Actor,
) -> Vec<String> {
    let server_shared_did_key = signing_key_from_pem(&state.secrets.atproto_private_key_pem)
        .ok()
        .map(|k| p256_to_did_key(k.verifying_key()));

    match (
        actor
            .at_rotation_key_pem
            .as_deref()
            .and_then(|pem| signing_key_from_pem(pem).ok())
            .map(|k| p256_to_did_key(k.verifying_key())),
        server_shared_did_key,
    ) {
        (Some(account_key), Some(shared_key)) => vec![account_key, shared_key],
        (Some(account_key), None) => vec![account_key],
        (None, Some(shared_key)) => vec![shared_key],
        (None, None) => vec![],
    }
}

/// `com.atproto.identity.requestPlcOperationSignature` — 登録メールへ確認コードを送る。
/// Phase Aゲート（`at_rotation_key_pem`必須）は`signPlcOperation`側でも再確認するが、
/// ここでも早期に弾く（コード送信自体を無駄にしないため）。
pub async fn xrpc_request_plc_operation_signature(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let Some(token) = extract_bearer(&headers) else {
        return ApiError::Unauthorized("Authorization ヘッダーが必要です").into_response();
    };
    let verified = match state
        .local_auth
        .verify_atp_access_token(token, &service_did(&state))
    {
        Ok(v) => v,
        Err(_) => return ApiError::Unauthorized("トークンが無効です").into_response(),
    };
    let actor = match state.actors.find_by_did(&verified.did).await {
        Ok(Some(a)) => a,
        _ => return ApiError::Unauthorized("アクターが見つかりません").into_response(),
    };
    if actor.at_rotation_key_pem.is_none() {
        return ApiError::BadRequest("ROTATION_KEY_NOT_MIGRATED".to_string()).into_response();
    }

    // `createSession`のauthFactorTokenと同じ原則: SMTP未設定インスタンスはコード送信自体が
    // 不可能なため、2FAごと丸々スキップする（`xrpc_sign_plc_operation`側もtoken検証を
    // 省略する）。コードの発行・メール送信は一切行わず空応答のみ返す。
    let smtp_settings = state.site_settings.get_all().await.unwrap_or_default();
    if !crate::mailer::is_smtp_configured(&smtp_settings) {
        return Json(serde_json::json!({})).into_response();
    }

    // メール爆撃対策: 同一actor+purposeで直近60秒以内に発行済みなら再送しない。
    let recent: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM email_short_codes
         WHERE actor_id = $1 AND purpose = $2 AND created_at > now() - interval '60 seconds'
         LIMIT 1",
    )
    .bind(actor.id)
    .bind(PLC_SIGNATURE_PURPOSE)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    if recent.is_some() {
        return Json(serde_json::json!({})).into_response();
    }

    let Some(login) = state
        .users
        .find_login_by_username(&actor.username)
        .await
        .ok()
        .flatten()
    else {
        return ApiError::Internal("ログイン情報が見つかりません".to_string()).into_response();
    };

    let code = format!("{:06}", uuid::Uuid::new_v4().as_u128() % 1_000_000);
    let code_hash = hex::encode(sha2::Sha256::digest(code.as_bytes()));
    let now = chrono::Utc::now();
    let id = seiran_common::generate_snowflake_id(now);
    if let Err(e) = state
        .email_short_codes
        .issue(
            id,
            actor.id,
            PLC_SIGNATURE_PURPOSE,
            &code_hash,
            now + chrono::Duration::minutes(15),
            now,
        )
        .await
    {
        return ApiError::Internal(format!(
            "[requestPlcOperationSignature] コード発行失敗: {}",
            e
        ))
        .into_response();
    }

    if let Err(e) =
        crate::mailer::send_plc_operation_signature_code(&smtp_settings, &login.email, &code).await
    {
        tracing::error!("[requestPlcOperationSignature] コード送信失敗: {}", e);
        // 送信自体に失敗した場合、ユーザーは届くはずのないコードの入力を永遠に求められる
        // ことになる。発行済みコードを取り消し、`xrpc_sign_plc_operation`側で
        // 「未発行＝検証不要」として扱わせる（SMTP未設定時と同じ扱いに帰着させる）。
        let _ = state
            .email_short_codes
            .revoke(actor.id, PLC_SIGNATURE_PURPOSE)
            .await;
    } else {
        tracing::info!(
            "[requestPlcOperationSignature] actor_id={} 確認コード送信完了",
            actor.id
        );
    }

    Json(serde_json::json!({})).into_response()
}

#[derive(Deserialize)]
pub struct SignPlcOperationRequest {
    pub token: String,
    #[serde(rename = "rotationKeys")]
    pub rotation_keys: Option<Vec<String>>,
    #[serde(rename = "alsoKnownAs")]
    pub also_known_as: Option<Vec<String>>,
    #[serde(rename = "verificationMethods")]
    pub verification_methods: Option<serde_json::Value>,
    pub services: Option<serde_json::Value>,
}

/// `com.atproto.identity.signPlcOperation` — 確認コードを検証し、要求された内容の
/// PLC更新オペレーションをアカウント単位ローテーションキーで署名して返す
/// （提出はしない、`submitPlcOperation`は別ステップ）。
pub async fn xrpc_sign_plc_operation(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<SignPlcOperationRequest>,
) -> impl IntoResponse {
    let Some(bearer) = extract_bearer(&headers) else {
        return ApiError::Unauthorized("Authorization ヘッダーが必要です").into_response();
    };
    let verified = match state
        .local_auth
        .verify_atp_access_token(bearer, &service_did(&state))
    {
        Ok(v) => v,
        Err(_) => return ApiError::Unauthorized("トークンが無効です").into_response(),
    };
    let actor = match state.actors.find_by_did(&verified.did).await {
        Ok(Some(a)) => a,
        _ => return ApiError::Unauthorized("アクターが見つかりません").into_response(),
    };
    let Some(rotation_key_pem) = actor.at_rotation_key_pem.as_deref() else {
        return ApiError::BadRequest("ROTATION_KEY_NOT_MIGRATED".to_string()).into_response();
    };

    // `xrpc_request_plc_operation_signature`と対の判定: 有効なコードが1件も発行されて
    // いなければ（SMTP未設定でそもそも発行していない、または発行後にメール送信自体が
    // 失敗して`revoke`済み）検証をスキップする。「SMTP設定の有無」ではなく「実際に
    // コードが存在するか」で判定することで、設定はあるのに送信が失敗した場合も
    // 同じ扱いに帰着させる。
    let has_pending = state
        .email_short_codes
        .has_pending(actor.id, PLC_SIGNATURE_PURPOSE)
        .await
        .unwrap_or(false);
    if has_pending {
        let code_hash = hex::encode(sha2::Sha256::digest(req.token.trim().as_bytes()));
        match state
            .email_short_codes
            .consume(actor.id, PLC_SIGNATURE_PURPOSE, &code_hash)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return ApiError::BadRequest("PLC_TOKEN_INVALID".to_string()).into_response()
            }
            Err(e) => {
                return ApiError::Internal(format!("[signPlcOperation] コード検証失敗: {}", e))
                    .into_response()
            }
        }
    }

    let (current_data, prev) =
        match fetch_current_plc_doc_and_prev(&verified.did, &state.http_client).await {
            Ok(v) => v,
            Err(e) => {
                return ApiError::BadGateway(format!("PLC_DIRECTORY_UNREACHABLE: {e}"))
                    .into_response()
            }
        };

    let signing_key = match signing_key_from_pem(rotation_key_pem) {
        Ok(k) => k,
        Err(e) => {
            return ApiError::Internal(format!("ローテーション鍵パース失敗: {e}")).into_response()
        }
    };

    let new_rotation_keys = req
        .rotation_keys
        .unwrap_or_else(|| current_rotation_key_dids(&state, &actor));

    let operation = match prepare_plc_rotation_update(
        &current_data,
        &prev,
        new_rotation_keys,
        req.also_known_as,
        req.verification_methods,
        req.services,
        &signing_key,
    ) {
        Ok(op) => op,
        Err(e) => {
            return ApiError::Internal(format!("[signPlcOperation] オペレーション生成失敗: {e}"))
                .into_response()
        }
    };

    Json(serde_json::json!({ "operation": operation })).into_response()
}

#[derive(Deserialize)]
pub struct SubmitPlcOperationRequest {
    pub operation: serde_json::Value,
}

/// `com.atproto.identity.submitPlcOperation` — 転出先PDSから渡された署名済みオペレー
/// ションをplc.directoryへ提出する（不可逆境界）。常に「トークンで認証されたDID」宛に
/// 提出する（リクエスト中にDIDヒントがあっても信用しない）。成功時に`#identity`/`#account`
/// イベントを発火し、`did_moved_out_at`をセットする。
pub async fn xrpc_submit_plc_operation(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<SubmitPlcOperationRequest>,
) -> impl IntoResponse {
    let Some(bearer) = extract_bearer(&headers) else {
        return ApiError::Unauthorized("Authorization ヘッダーが必要です").into_response();
    };
    let verified = match state
        .local_auth
        .verify_atp_access_token(bearer, &service_did(&state))
    {
        Ok(v) => v,
        Err(_) => return ApiError::Unauthorized("トークンが無効です").into_response(),
    };
    let actor = match state.actors.find_by_did(&verified.did).await {
        Ok(Some(a)) => a,
        _ => return ApiError::Unauthorized("アクターが見つかりません").into_response(),
    };

    if let Err(e) = seiran_common::atp::plc::submit_plc_operation_raw(
        &verified.did,
        &req.operation,
        &state.http_client,
    )
    .await
    {
        tracing::error!(
            "[submitPlcOperation] plc.directory提出失敗 actor_id={} did={}: {}",
            actor.id,
            verified.did,
            e
        );
        return ApiError::BadGateway(format!("PLC_SUBMIT_FAILED: {e}")).into_response();
    }

    // ─────────────────────────────────────────────────────────────
    // ★不可逆境界: ここから先、このDIDのservice endpointはseiranを離れる。
    // ─────────────────────────────────────────────────────────────
    let now = chrono::Utc::now();
    let handle = format!("{}.{}", actor.username, state.local_domain);

    if let Err(e) = state
        .atp_service
        .broadcast_identity_event(actor.id, &verified.did, &handle, now)
        .await
    {
        tracing::error!(
            "[submitPlcOperation] #identity broadcast失敗（PLC提出は成功済み） actor_id={}: {:?}",
            actor.id,
            e
        );
    }
    if let Err(e) = state
        .atp_service
        .broadcast_account_event(
            actor.id,
            &verified.did,
            &handle,
            now,
            false,
            Some("deactivated"),
        )
        .await
    {
        tracing::error!(
            "[submitPlcOperation] #account broadcast失敗（PLC提出は成功済み） actor_id={}: {:?}",
            actor.id,
            e
        );
    }
    if let Err(e) = sqlx::query(
        "UPDATE actors SET did_moved_out_at = COALESCE(did_moved_out_at, $1) WHERE id = $2",
    )
    .bind(now)
    .bind(actor.id)
    .execute(&state.db)
    .await
    {
        tracing::error!(
            "[submitPlcOperation] did_moved_out_at設定失敗（DIDは既にseiranを離れました！） actor_id={}: {}",
            actor.id,
            e
        );
    }

    tracing::warn!(
        "[submitPlcOperation] actor_id={} did={} 転出完了",
        actor.id,
        verified.did
    );

    Json(serde_json::json!({})).into_response()
}

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use p256::pkcs8::{EncodePublicKey, LineEnding};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;

use seiran_common::atp::{cid_from_sha256_hex, cid_to_string, resolve_atproto_verification_key};

use crate::AppState;

/// `com.atproto.repo.uploadBlob` のサービス間認証JWTクレーム。
#[derive(Deserialize)]
struct UploadBlobClaims {
    #[allow(dead_code)]
    iss: String,
    #[allow(dead_code)]
    aud: String,
    lxm: String,
}

/// `Authorization: Bearer <jwt>` の `iss` クレームだけを署名検証前に読む
/// （検証鍵を `iss` のDIDから解決する必要があるため）。
fn peek_unverified_iss(jwt: &str) -> Option<String> {
    let payload_b64 = jwt.split('.').nth(1)?;
    let payload_bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;
    value.get("iss").and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// `com.atproto.repo.uploadBlob` 受け口。
///
/// Bsky公式動画パイプライン（`app.bsky.video.uploadVideo`）がトランスコード完了後に
/// 呼び戻してくるコールバック。`Authorization` のサービス間認証JWTを検証した上で、
/// 受信バイト列のSHA-256からCIDを計算して返す。**受信バイト列自体はS3に保存せず
/// 読み捨てる**（ローカル/Fedi配信は常にアップロード時のオリジナルファイルを使うため
/// このコピーは不要。詳細は docs/03_multi_protocol_engine_specification.md §12）。
pub async fn xrpc_upload_blob(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> impl IntoResponse {
    let Some(auth_header) = headers.get("authorization").and_then(|v| v.to_str().ok()) else {
        return (StatusCode::UNAUTHORIZED, "Authorization ヘッダがありません").into_response();
    };
    let Some(jwt) = auth_header.strip_prefix("Bearer ") else {
        return (StatusCode::UNAUTHORIZED, "Bearer トークンが必要です").into_response();
    };

    let Some(iss) = peek_unverified_iss(jwt) else {
        return (StatusCode::UNAUTHORIZED, "JWTのissクレームを読み取れません").into_response();
    };

    let verifying_key = match resolve_atproto_verification_key(&iss, &state.ap_client.http).await {
        Ok(k) => k,
        Err(e) => {
            eprintln!("[uploadBlob] 検証鍵解決失敗 iss={}: {}", iss, e);
            return (StatusCode::UNAUTHORIZED, "検証鍵の解決に失敗しました").into_response();
        }
    };
    let public_key_pem = match verifying_key.to_public_key_pem(LineEnding::LF) {
        Ok(pem) => pem,
        Err(e) => {
            eprintln!("[uploadBlob] 公開鍵PEM変換失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "内部エラー").into_response();
        }
    };
    let decoding_key = match DecodingKey::from_ec_pem(public_key_pem.as_bytes()) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("[uploadBlob] DecodingKey構築失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "内部エラー").into_response();
        }
    };

    let expected_aud = format!("did:web:{}", state.local_domain);
    let mut validation = Validation::new(Algorithm::ES256);
    validation.set_audience(&[&expected_aud]);

    let claims = match jsonwebtoken::decode::<UploadBlobClaims>(jwt, &decoding_key, &validation) {
        Ok(data) => data.claims,
        Err(e) => {
            eprintln!("[uploadBlob] JWT検証失敗 iss={}: {}", iss, e);
            return (StatusCode::UNAUTHORIZED, "JWT検証に失敗しました").into_response();
        }
    };
    if claims.lxm != "com.atproto.repo.uploadBlob" {
        eprintln!("[uploadBlob] lxm不一致: {}", claims.lxm);
        return (StatusCode::UNAUTHORIZED, "lxmが一致しません").into_response();
    }

    let mime_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let sha256_hex = hex::encode(Sha256::digest(&body));
    let cid = match cid_from_sha256_hex(&sha256_hex) {
        Ok(c) => cid_to_string(&c),
        Err(e) => {
            eprintln!("[uploadBlob] CID生成失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "CID生成失敗").into_response();
        }
    };

    eprintln!("[uploadBlob] 検証OK iss={} cid={} size={}（読み捨て・S3保存なし）", iss, cid, body.len());

    Json(serde_json::json!({
        "blob": {
            "$type": "blob",
            "ref": { "$link": cid },
            "mimeType": mime_type,
            "size": body.len(),
        }
    })).into_response()
}

#[derive(Deserialize)]
pub struct GetRecordParams {
    pub repo: String,
    pub collection: String,
    pub rkey: String,
}

#[derive(Serialize)]
pub struct GetRecordResponse {
    pub uri: String,
    pub cid: String,
    pub value: serde_json::Value,
}

pub async fn xrpc_get_record(
    Query(params): Query<GetRecordParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // 投稿は専用パス
    if params.collection == "app.bsky.feed.post" {
        return get_record_post(&params, &state).await;
    }

    // それ以外は atp_records + atp_blocks から返す
    get_record_from_atp_records(&params, &state).await
}

async fn get_record_post(params: &GetRecordParams, state: &AppState) -> axum::response::Response {
    let record = match state.posts.find_record(&params.repo, &params.rkey).await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, "レコードが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[getRecord] DB エラー: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let value = serde_json::json!({
        "$type": "app.bsky.feed.post",
        "text": record.body,
        "createdAt": record.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });

    Json(GetRecordResponse { uri: record.at_uri, cid: record.at_cid, value }).into_response()
}

/// `atp_records` テーブルから CID を引き、`atp_blocks` テーブルの CBOR を JSON にして返す。
async fn get_record_from_atp_records(
    params: &GetRecordParams,
    state: &AppState,
) -> axum::response::Response {
    // repo（DID または handle）からアクター取得
    let actor = match state.actors.find_by_did(&params.repo).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            // did: でなければ username として検索（ローカルアクター）
            match state.actors.find_by_username_domain(&params.repo, &state.local_domain).await {
                Ok(Some(a)) => a,
                _ => return (StatusCode::NOT_FOUND, "リポジトリが見つかりません").into_response(),
            }
        }
        Err(e) => {
            eprintln!("[getRecord] アクター取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // atp_records から CID を取得
    let record_row = sqlx::query(
        "SELECT cid FROM atp_records
         WHERE actor_id = $1 AND collection = $2 AND rkey = $3 LIMIT 1",
    )
    .bind(actor.id)
    .bind(&params.collection)
    .bind(&params.rkey)
    .fetch_optional(&state.db)
    .await;

    let cid_str = match record_row {
        Ok(Some(row)) => row.try_get::<String, _>("cid").unwrap_or_default(),
        Ok(None) => return (StatusCode::NOT_FOUND, "レコードが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[getRecord] atp_records 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // atp_blocks から CBOR バイト列を取得
    let block_row = sqlx::query(
        "SELECT bytes FROM atp_blocks WHERE cid = $1 AND actor_id = $2 LIMIT 1",
    )
    .bind(&cid_str)
    .bind(actor.id)
    .fetch_optional(&state.db)
    .await;

    let cbor_bytes: Vec<u8> = match block_row {
        Ok(Some(row)) => row.try_get::<Vec<u8>, _>("bytes").unwrap_or_default(),
        Ok(None) => return (StatusCode::NOT_FOUND, "ブロックが見つかりません").into_response(),
        Err(e) => {
            eprintln!("[getRecord] atp_blocks 取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    // DAG-CBOR → serde_json::Value
    let value: serde_json::Value = match serde_ipld_dagcbor::from_slice(&cbor_bytes) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[getRecord] CBOR デコード失敗 (cid={}): {}", cid_str, e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "CBOR デコード失敗").into_response();
        }
    };

    let at_did = actor.at_did.as_deref().unwrap_or(&params.repo);
    let uri = format!("at://{}/{}/{}", at_did, params.collection, params.rkey);

    Json(GetRecordResponse { uri, cid: cid_str, value }).into_response()
}

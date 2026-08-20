use axum::{
    body::Bytes,
    extract::{Query, State},
    http::HeaderMap,
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

use seiran_common::atp::{
    cid_from_sha256_hex, cid_from_str, cid_to_string, fetch_raw_did_document, generate_tid,
    resolve_atproto_verification_key,
};
use seiran_common::repository::Actor;
use seiran_common::{
    ext_for_mime_type, generate_snowflake_id, select_provider, sniff_mime_type, S3StorageClient,
};
use uuid::Uuid;

use super::{extract_bearer, service_did};
use crate::error::ApiError;
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
    value
        .get("iss")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
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
        return ApiError::Unauthorized("Authorization ヘッダがありません").into_response();
    };
    let Some(jwt) = auth_header.strip_prefix("Bearer ") else {
        return ApiError::Unauthorized("Bearer トークンが必要です").into_response();
    };

    let Some(iss) = peek_unverified_iss(jwt) else {
        return ApiError::Unauthorized("JWTのissクレームを読み取れません").into_response();
    };

    let verifying_key = match resolve_atproto_verification_key(&iss, &state.ap_client.http).await {
        Ok(k) => k,
        Err(e) => {
            tracing::error!("[uploadBlob] 検証鍵解決失敗 iss={}: {}", iss, e);
            return ApiError::Unauthorized("検証鍵の解決に失敗しました").into_response();
        }
    };
    let public_key_pem = match verifying_key.to_public_key_pem(LineEnding::LF) {
        Ok(pem) => pem,
        Err(e) => {
            return ApiError::Internal(format!("[uploadBlob] 公開鍵PEM変換失敗: {}", e))
                .into_response();
        }
    };
    let decoding_key = match DecodingKey::from_ec_pem(public_key_pem.as_bytes()) {
        Ok(k) => k,
        Err(e) => {
            return ApiError::Internal(format!("[uploadBlob] DecodingKey構築失敗: {}", e))
                .into_response();
        }
    };

    let expected_aud = format!("did:web:{}", state.local_domain);
    let mut validation = Validation::new(Algorithm::ES256);
    validation.set_audience(&[&expected_aud]);

    let claims = match jsonwebtoken::decode::<UploadBlobClaims>(jwt, &decoding_key, &validation) {
        Ok(data) => data.claims,
        Err(e) => {
            tracing::error!("[uploadBlob] JWT検証失敗 iss={}: {}", iss, e);
            return ApiError::Unauthorized("JWT検証に失敗しました").into_response();
        }
    };
    if claims.lxm != "com.atproto.repo.uploadBlob" {
        tracing::info!("[uploadBlob] lxm不一致: {}", claims.lxm);
        return ApiError::Unauthorized("lxmが一致しません").into_response();
    }

    // Content-Type ヘッダーをそのまま信用しない。Bsky公式動画パイプラインからの代理POSTは
    // 実機確認で `Content-Type: */*` という無効なワイルドカード値を送ってくることがあり
    // （2026-07-17 マイケル実機確認）、そのまま保存すると getBlob が返す動画の
    // Content-Type も `*/*` になって再生できなくなる。ヘッダーがワイルドカードや欠落の
    // 場合はマジックバイトから実際の MIME type を判定する。
    let header_mime = headers.get("content-type").and_then(|v| v.to_str().ok());
    let mime_type = match header_mime {
        Some(m) if !m.is_empty() && !m.contains('*') => m.to_string(),
        _ => sniff_mime_type(&body, "application/octet-stream"),
    };

    let sha256_hex = hex::encode(Sha256::digest(&body));
    let cid = match cid_from_sha256_hex(&sha256_hex) {
        Ok(c) => cid_to_string(&c),
        Err(e) => {
            return ApiError::Internal(format!("[uploadBlob] CID生成失敗: {}", e)).into_response();
        }
    };

    // 受信バイト列を実際に S3 へ保存する。以前は「ローカル/Fedi配信は常にアップロード時の
    // オリジナルファイルを使うため不要」として読み捨てていたが、Bsky公式動画パイプライン
    // （video.bsky.app）はトランスコード完了後にこのエンドポイントへ代理POSTしてきており、
    // 後で video.bsky.app 自身（または視聴者）がこの CID を getBlob で取得しようとすると
    // 404 になり動画が再生できない不具合の直接原因だった（2026-07-17 マイケル実機確認）。
    match state.actors.find_by_did(&iss).await {
        Ok(Some(actor)) => {
            if let Err(e) = store_uploaded_blob(
                &state,
                actor.id,
                &sha256_hex,
                &cid,
                &mime_type,
                body.len() as i64,
                &body,
            )
            .await
            {
                tracing::error!(
                    "[uploadBlob] blob保存失敗（読み捨てて続行）cid={}: {}",
                    cid,
                    e
                );
            }
        }
        Ok(None) => tracing::warn!(
            "[uploadBlob] iss={} のアクターが見つからずblob保存スキップ cid={}",
            iss,
            cid
        ),
        Err(e) => tracing::error!("[uploadBlob] アクター解決失敗 iss={}: {}", iss, e),
    }

    tracing::info!(
        "[uploadBlob] 検証OK iss={} cid={} size={}",
        iss,
        cid,
        body.len()
    );

    Json(serde_json::json!({
        "blob": {
            "$type": "blob",
            "ref": { "$link": cid },
            "mimeType": mime_type,
            "size": body.len(),
        }
    }))
    .into_response()
}

/// `uploadBlob` で受信したバイト列を `atp_blobs` テーブル経由で S3 に保存する。
/// 既に同じ SHA-256（= 同じ内容）が保存済みならスキップする（content-addressable な
/// ので重複排除で十分。動画パイプラインが複数アカウント分の同一トランスコード結果を
/// 提出してくるケースもこれで安全）。
async fn store_uploaded_blob(
    state: &AppState,
    actor_id: i64,
    sha256_hex: &str,
    cid: &str,
    mime_type: &str,
    size: i64,
    body: &[u8],
) -> Result<(), String> {
    // 進行中の動画パイプラインジョブに対応するコールバックのみを受理する。これが無いと、
    // 正当な自己署名JWT（DID本人なら誰でも作れる）さえあれば無制限回数・任意サイズで
    // S3を消費できてしまう（2026-07-17 マイケル指摘）。
    let has_pending_job: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM media_files WHERE uploaded_by_actor_id = $1 AND bsky_video_status = 'pending')",
    )
    .bind(actor_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| format!("pendingジョブ確認失敗: {}", e))?;
    if !has_pending_job {
        return Err("進行中の動画パイプラインジョブが無いため保存を拒否".to_string());
    }

    // 既に media_files 側に同じバイト列があれば S3 への重複保存を避ける
    // （getBlob は media_files/atp_blobs 両方を検索するので、atp_blobs 側への
    // 新規保存をスキップしても解決可能なまま。2026-07-17 マイケル指摘）。
    let in_media_files: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media_files WHERE sha256 = $1)")
            .bind(sha256_hex)
            .fetch_one(&state.db)
            .await
            .map_err(|e| format!("media_files重複チェック失敗: {}", e))?;
    if in_media_files {
        return Ok(());
    }

    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM atp_blobs WHERE sha256 = $1)")
            .bind(sha256_hex)
            .fetch_one(&state.db)
            .await
            .map_err(|e| format!("既存チェック失敗: {}", e))?;
    if exists {
        return Ok(());
    }

    let provider = select_provider(state.storage_providers.as_ref(), size)
        .await
        .map_err(|e| format!("プロバイダー選択失敗: {}", e))?;
    let ext = ext_for_mime_type(mime_type);
    let storage_key = format!("blobs/{}.{}", Uuid::new_v4(), ext);
    let s3 = S3StorageClient::new(&provider);
    s3.put(&storage_key, body.to_vec(), mime_type)
        .await
        .map_err(|e| format!("S3アップロード失敗: {}", e))?;

    let id = generate_snowflake_id(chrono::Utc::now());
    sqlx::query(
        "INSERT INTO atp_blobs (id, actor_id, sha256, cid, mime_type, size, storage_provider_id, storage_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (sha256) DO NOTHING",
    )
    .bind(id)
    .bind(actor_id)
    .bind(sha256_hex)
    .bind(cid)
    .bind(mime_type)
    .bind(size)
    .bind(provider.id)
    .bind(&storage_key)
    .execute(&state.db)
    .await
    .map_err(|e| format!("atp_blobs INSERT失敗: {}", e))?;

    Ok(())
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
        Ok(None) => return ApiError::NotFound("レコードが見つかりません").into_response(),
        Err(e) => {
            return ApiError::Internal(format!("[getRecord] DB エラー: {}", e)).into_response();
        }
    };

    // 実際にコミット済みのレコード（embed 等を含む完全な内容）を atp_blocks から
    // 取得してデコードする。以前は posts.body/created_at だけからその場で JSON を
    // 再構築しており、embed（画像・動画・引用等）を一切返していなかった
    // （2026-07-17 マイケル指摘で発覚。実際の firehose 配信・relay 検証には
    // 影響しない表示専用のバグだった）。
    let actor = state.actors.find_by_did(&params.repo).await.ok().flatten();
    let actor = match actor {
        Some(a) => Some(a),
        None => state
            .actors
            .find_by_username_domain(&params.repo, &state.local_domain)
            .await
            .ok()
            .flatten(),
    };

    if let Some(actor) = actor {
        let block_row =
            sqlx::query("SELECT bytes FROM atp_blocks WHERE cid = $1 AND actor_id = $2 LIMIT 1")
                .bind(&record.at_cid)
                .bind(actor.id)
                .fetch_optional(&state.db)
                .await;
        if let Ok(Some(row)) = block_row {
            let cbor_bytes: Vec<u8> = row.try_get("bytes").unwrap_or_default();
            match serde_ipld_dagcbor::from_slice::<ipld_core::ipld::Ipld>(&cbor_bytes) {
                Ok(ipld) => {
                    let value = ipld_to_json(&ipld);
                    return Json(GetRecordResponse {
                        uri: record.at_uri,
                        cid: record.at_cid,
                        value,
                    })
                    .into_response();
                }
                Err(e) => {
                    tracing::error!(
                        "[getRecord] CBOR デコード失敗 (cid={}): {}",
                        record.at_cid,
                        e
                    );
                }
            }
        }
    }

    // フォールバック: atp_blocks から取得できなかった場合のみ、簡易再構築する
    // （embed は失われるが、text/createdAt だけでも返した方がまし）。
    let value = serde_json::json!({
        "$type": "app.bsky.feed.post",
        "text": record.body,
        "createdAt": record.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });

    Json(GetRecordResponse {
        uri: record.at_uri,
        cid: record.at_cid,
        value,
    })
    .into_response()
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
            match state
                .actors
                .find_by_username_domain(&params.repo, &state.local_domain)
                .await
            {
                Ok(Some(a)) => a,
                _ => return ApiError::NotFound("リポジトリが見つかりません").into_response(),
            }
        }
        Err(e) => {
            return ApiError::Internal(format!("[getRecord] アクター取得失敗: {}", e))
                .into_response();
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
        Ok(None) => return ApiError::NotFound("レコードが見つかりません").into_response(),
        Err(e) => {
            return ApiError::Internal(format!("[getRecord] atp_records 取得失敗: {}", e))
                .into_response();
        }
    };

    // atp_blocks から CBOR バイト列を取得
    let block_row =
        sqlx::query("SELECT bytes FROM atp_blocks WHERE cid = $1 AND actor_id = $2 LIMIT 1")
            .bind(&cid_str)
            .bind(actor.id)
            .fetch_optional(&state.db)
            .await;

    let cbor_bytes: Vec<u8> = match block_row {
        Ok(Some(row)) => row.try_get::<Vec<u8>, _>("bytes").unwrap_or_default(),
        Ok(None) => return ApiError::NotFound("ブロックが見つかりません").into_response(),
        Err(e) => {
            return ApiError::Internal(format!("[getRecord] atp_blocks 取得失敗: {}", e))
                .into_response();
        }
    };

    // DAG-CBOR → Ipld → serde_json::Value（CID リンクを含みうるため Ipld 経由で変換する）
    let ipld: ipld_core::ipld::Ipld = match serde_ipld_dagcbor::from_slice(&cbor_bytes) {
        Ok(v) => v,
        Err(e) => {
            return ApiError::Internal(format!(
                "[getRecord] CBOR デコード失敗 (cid={}): {}",
                cid_str, e
            ))
            .into_response();
        }
    };
    let value = ipld_to_json(&ipld);

    let at_did = actor.at_did.as_deref().unwrap_or(&params.repo);
    let uri = format!("at://{}/{}/{}", at_did, params.collection, params.rkey);

    Json(GetRecordResponse {
        uri,
        cid: cid_str,
        value,
    })
    .into_response()
}

/// `repo` パラメータ（DID または `{username}.{local_domain}` ハンドル）からアクターを解決する。
/// `listRecords`/`describeRepo` など、アクターが必須で見つからなければ 404 を返すエンドポイント用
/// （`getRecord` のような「見つからなくても簡易フォールバックする」経路とは異なる）。
pub(crate) async fn resolve_repo_actor(
    state: &AppState,
    repo: &str,
) -> Result<Actor, axum::response::Response> {
    match state.actors.find_by_did(repo).await {
        Ok(Some(a)) => Ok(a),
        Ok(None) => match state
            .actors
            .find_by_username_domain(repo, &state.local_domain)
            .await
        {
            Ok(Some(a)) => Ok(a),
            Ok(None) => Err(ApiError::NotFound("リポジトリが見つかりません").into_response()),
            Err(e) => Err(
                ApiError::Internal(format!("[repo解決] アクター取得失敗: {}", e)).into_response(),
            ),
        },
        Err(e) => {
            Err(ApiError::Internal(format!("[repo解決] アクター取得失敗: {}", e)).into_response())
        }
    }
}

/// `atp_blocks` から CID のレコードを取得し DAG-CBOR → JSON にデコードする。
/// 見つからない・デコード失敗の場合は `None`（呼び出し元は空オブジェクト等でフォールバックする）。
async fn fetch_record_value(
    state: &AppState,
    actor_id: i64,
    cid_str: &str,
) -> Option<serde_json::Value> {
    let row = sqlx::query("SELECT bytes FROM atp_blocks WHERE cid = $1 AND actor_id = $2 LIMIT 1")
        .bind(cid_str)
        .bind(actor_id)
        .fetch_optional(&state.db)
        .await
        .ok()??;
    let cbor_bytes: Vec<u8> = row.try_get("bytes").ok()?;
    let ipld: ipld_core::ipld::Ipld = serde_ipld_dagcbor::from_slice(&cbor_bytes).ok()?;
    Some(ipld_to_json(&ipld))
}

#[derive(Deserialize)]
pub struct ListRecordsParams {
    pub repo: String,
    pub collection: String,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    #[serde(default)]
    pub reverse: bool,
}

#[derive(Serialize)]
struct ListRecordsEntry {
    uri: String,
    cid: String,
    value: serde_json::Value,
}

/// `com.atproto.repo.listRecords` — 指定コレクションのレコード一覧を rkey 順にページングして返す。
/// サードパーティのインデクサー（Clearsky等）はこのエンドポイントで PDS から直接投稿履歴を
/// 取得するため、未実装だと「投稿が1件も見えないユーザー」として扱われる
/// （2026-08-20 マイケル報告で発覚。firehose 配信自体は正常だった）。
pub async fn xrpc_list_records(
    Query(params): Query<ListRecordsParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor = match resolve_repo_actor(&state, &params.repo).await {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let limit = params.limit.unwrap_or(50).clamp(1, 100);
    let at_did = actor.at_did.clone().unwrap_or_else(|| params.repo.clone());

    let rows: Vec<(String, String)> = if params.collection == "app.bsky.feed.post" {
        match state
            .posts
            .list_records_by_actor(actor.id, limit, params.cursor.as_deref(), params.reverse)
            .await
        {
            Ok(recs) => recs.into_iter().map(|r| (r.at_rkey, r.at_cid)).collect(),
            Err(e) => {
                return ApiError::Internal(format!("[listRecords] DB エラー: {}", e))
                    .into_response()
            }
        }
    } else {
        match state
            .atp_repo
            .list_records(
                actor.id,
                &params.collection,
                limit,
                params.cursor.as_deref(),
                params.reverse,
            )
            .await
        {
            Ok(recs) => recs,
            Err(e) => {
                return ApiError::Internal(format!("[listRecords] DB エラー: {}", e))
                    .into_response()
            }
        }
    };

    let next_cursor = rows.last().map(|(rkey, _)| rkey.clone());

    let mut records = Vec::with_capacity(rows.len());
    for (rkey, cid) in rows {
        let value = fetch_record_value(&state, actor.id, &cid)
            .await
            .unwrap_or_else(|| serde_json::json!({}));
        records.push(ListRecordsEntry {
            uri: format!("at://{}/{}/{}", at_did, params.collection, rkey),
            cid,
            value,
        });
    }

    Json(serde_json::json!({
        "records": records,
        "cursor": next_cursor,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct DescribeRepoParams {
    pub repo: String,
}

/// `com.atproto.repo.describeRepo` — ハンドル・DID・保持コレクション一覧・DIDドキュメントを返す。
/// `pdsls.dev` 等の ATP 汎用エクスプローラーは通常このエンドポイントでコレクション一覧を得てから
/// `listRecords` を呼ぶため、`listRecords` と対で実装する。
pub async fn xrpc_describe_repo(
    Query(params): Query<DescribeRepoParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor = match resolve_repo_actor(&state, &params.repo).await {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let did = match actor.at_did.clone() {
        Some(d) => d,
        None => {
            return ApiError::NotFound("このアクターはATPリポジトリを持ちません").into_response()
        }
    };
    let handle = format!("{}.{}", actor.username, state.local_domain);

    let mut collections = match state.atp_repo.list_collections(actor.id).await {
        Ok(c) => c,
        Err(e) => {
            return ApiError::Internal(format!("[describeRepo] コレクション取得失敗: {}", e))
                .into_response()
        }
    };
    let has_posts: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM posts WHERE actor_id = $1 AND deleted_at IS NULL AND at_rkey IS NOT NULL)",
    )
    .bind(actor.id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false);
    if has_posts && !collections.iter().any(|c| c == "app.bsky.feed.post") {
        collections.push("app.bsky.feed.post".to_string());
    }
    collections.sort();

    let did_doc = fetch_raw_did_document(&did, &state.ap_client.http)
        .await
        .ok();

    Json(serde_json::json!({
        "handle": handle,
        "did": did,
        "didDoc": did_doc,
        "collections": collections,
        "handleIsCorrect": true,
    }))
    .into_response()
}

/// ATPセッション（`Authorization: Bearer <accessJwt>`、`com.atproto.server.createSession` で
/// 発行したもの）を検証し、`repo`パラメータが認証されたDIDと一致するアクターを返す。
/// `createRecord`/`putRecord`/`deleteRecord`/`applyWrites` の共通認証ゲート
/// （他人のリポジトリへの書き込みを防ぐ）。
async fn authenticate_atp_write(
    state: &AppState,
    headers: &HeaderMap,
    repo_param: &str,
) -> Result<Actor, axum::response::Response> {
    let Some(token) = extract_bearer(headers) else {
        return Err(ApiError::Unauthorized("Authorization ヘッダーが必要です").into_response());
    };
    let verified = state
        .local_auth
        .verify_atp_access_token(token, &service_did(state))
        .map_err(|_| ApiError::Unauthorized("トークンが無効です").into_response())?;

    let actor = resolve_repo_actor(state, repo_param).await?;
    let Some(actor_did) = actor.at_did.as_deref() else {
        return Err(
            ApiError::Unauthorized("このアクターはATPリポジトリを持ちません").into_response(),
        );
    };
    if actor_did != verified.did {
        return Err(ApiError::Forbidden("他のリポジトリへは書き込めません").into_response());
    }
    Ok(actor)
}

/// `putRecord`/`createRecord`/`applyWrites` が `app.bsky.actor.profile` を受け取った際、
/// `actors`テーブル（`display_name`/`bio`/`avatar_media_id`/`banner_media_id`）にも反映する。
/// ATPリポジトリ（`atp_blocks`/`atp_records`）は`commit_generic_record`が既に更新済みで、
/// これとは別データソースである`actors`（seiranのUI/APIが表示に使う値）を追従させるための
/// 「ATP→actors」逆方向同期（`AtpCommitService::commit_profile`の「actors→ATP」とは逆向き）。
/// ローカルユーザーでない場合、または`avatar`/`banner`のCIDに対応する`media_files`行が
/// まだ存在しない場合（bsky.app等から新規アップロードされ、seiran側に未取り込みの画像）は
/// 該当フィールドを更新せず既存値を維持する。
async fn sync_profile_to_actors(state: &AppState, actor: &Actor, value: &serde_json::Value) {
    let Some(user_id) = actor.user_id else {
        return;
    };
    let Ok(Some(current)) = state.actors.find_profile_by_user_id(user_id).await else {
        return;
    };

    let display_name = value
        .get("displayName")
        .and_then(|v| v.as_str())
        .or(current.display_name.as_deref());
    let bio = value
        .get("description")
        .and_then(|v| v.as_str())
        .or(current.bio.as_deref());
    let avatar_media_id = resolve_blob_media_id(state, value.get("avatar"), actor.id)
        .await
        .or(current.avatar_media_id);
    let banner_media_id = resolve_blob_media_id(state, value.get("banner"), actor.id)
        .await
        .or(current.banner_media_id);
    let emoji_map = current
        .emoji_map
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));

    if let Err(e) = state
        .actors
        .update_profile(
            user_id,
            display_name,
            bio,
            avatar_media_id,
            banner_media_id,
            &current.profile_fields,
            &emoji_map,
        )
        .await
    {
        tracing::error!(
            "[putRecord] app.bsky.actor.profile → actors 同期失敗 actor_id={}: {}",
            actor.id,
            e
        );
    }
}

/// blob参照（`{"$type":"blob","ref":{"$link":"<CID>"},...}`）のCIDから、対応する
/// `media_files` 行（既にseiran経由でアップロード済みの画像）を検索する。CIDのmultihashは
/// sha256そのものなので、逆算して一致検索する（`xrpc_get_blob`と同じ手法）。
async fn resolve_blob_media_id(
    state: &AppState,
    blob_value: Option<&serde_json::Value>,
    actor_id: i64,
) -> Option<i64> {
    let cid_str = blob_value?.get("ref")?.get("$link")?.as_str()?;
    let cid = cid_from_str(cid_str).ok()?;
    let mh = cid.hash();
    if mh.code() != 0x12 {
        return None;
    }
    let sha256_hex = hex::encode(mh.digest());
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM media_files WHERE sha256 = $1 AND uploaded_by_actor_id = $2 LIMIT 1",
    )
    .bind(&sha256_hex)
    .bind(actor_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
}

/// `app.bsky.feed.post` は画像/動画embed組み立て等の専用ロジック（`AtpCommitService::commit_post`、
/// `posts`テーブル更新込み）を経由する必要があり、汎用書き込みエンドポイントでは扱えない。
fn reject_post_collection(collection: &str) -> Option<axum::response::Response> {
    if collection == "app.bsky.feed.post" {
        Some(
            ApiError::BadRequest(
                "app.bsky.feed.post はseiranネイティブの投稿APIを使用してください".to_string(),
            )
            .into_response(),
        )
    } else {
        None
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRecordRequest {
    pub repo: String,
    pub collection: String,
    pub rkey: Option<String>,
    pub record: serde_json::Value,
}

/// `com.atproto.repo.createRecord` — 新規レコードを作成する（`rkey`省略時はTIDを自動生成）。
pub async fn xrpc_create_record(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateRecordRequest>,
) -> impl IntoResponse {
    if let Some(resp) = reject_post_collection(&req.collection) {
        return resp;
    }
    let actor = match authenticate_atp_write(&state, &headers, &req.repo).await {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let at_did = actor
        .at_did
        .clone()
        .expect("authenticate_atp_writeで確認済み");
    let rkey = req.rkey.unwrap_or_else(generate_tid);

    match state
        .atp_service
        .commit_generic_record(
            actor.id,
            req.collection.clone(),
            rkey.clone(),
            &req.record,
            "create",
            chrono::Utc::now(),
        )
        .await
    {
        Ok((_, record_cid)) => {
            if req.collection == "app.bsky.actor.profile" {
                sync_profile_to_actors(&state, &actor, &req.record).await;
            }
            Json(serde_json::json!({
                "uri": format!("at://{}/{}/{}", at_did, req.collection, rkey),
                "cid": cid_to_string(&record_cid),
            }))
            .into_response()
        }
        Err(e) => ApiError::Internal(format!("[createRecord] コミット失敗: {}", e)).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PutRecordRequest {
    pub repo: String,
    pub collection: String,
    pub rkey: String,
    pub record: serde_json::Value,
}

/// `com.atproto.repo.putRecord` — `rkey`を指定してレコードを作成/上書きする。
pub async fn xrpc_put_record(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<PutRecordRequest>,
) -> impl IntoResponse {
    if let Some(resp) = reject_post_collection(&req.collection) {
        return resp;
    }
    let actor = match authenticate_atp_write(&state, &headers, &req.repo).await {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let at_did = actor
        .at_did
        .clone()
        .expect("authenticate_atp_writeで確認済み");

    match state
        .atp_service
        .commit_generic_record(
            actor.id,
            req.collection.clone(),
            req.rkey.clone(),
            &req.record,
            "update",
            chrono::Utc::now(),
        )
        .await
    {
        Ok((_, record_cid)) => {
            if req.collection == "app.bsky.actor.profile" {
                sync_profile_to_actors(&state, &actor, &req.record).await;
            }
            Json(serde_json::json!({
                "uri": format!("at://{}/{}/{}", at_did, req.collection, req.rkey),
                "cid": cid_to_string(&record_cid),
            }))
            .into_response()
        }
        Err(e) => ApiError::Internal(format!("[putRecord] コミット失敗: {}", e)).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteRecordRequest {
    pub repo: String,
    pub collection: String,
    pub rkey: String,
}

/// `com.atproto.repo.deleteRecord` — レコードを削除する。
pub async fn xrpc_delete_record(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<DeleteRecordRequest>,
) -> impl IntoResponse {
    if let Some(resp) = reject_post_collection(&req.collection) {
        return resp;
    }
    let actor = match authenticate_atp_write(&state, &headers, &req.repo).await {
        Ok(a) => a,
        Err(resp) => return resp,
    };

    match state
        .atp_service
        .delete_atp_record_generic(actor.id, &req.collection, &req.rkey, chrono::Utc::now())
        .await
    {
        Ok(()) => Json(serde_json::json!({})).into_response(),
        Err(e) => ApiError::Internal(format!("[deleteRecord] 削除失敗: {}", e)).into_response(),
    }
}

/// `com.atproto.repo.applyWrites` の `writes[]` 各要素。
#[derive(Deserialize)]
#[serde(tag = "$type")]
pub enum ApplyWritesOp {
    #[serde(rename = "com.atproto.repo.applyWrites#create")]
    Create {
        collection: String,
        rkey: Option<String>,
        value: serde_json::Value,
    },
    #[serde(rename = "com.atproto.repo.applyWrites#update")]
    Update {
        collection: String,
        rkey: String,
        value: serde_json::Value,
    },
    #[serde(rename = "com.atproto.repo.applyWrites#delete")]
    Delete { collection: String, rkey: String },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyWritesRequest {
    pub repo: String,
    pub writes: Vec<ApplyWritesOp>,
}

/// `com.atproto.repo.applyWrites` — 複数レコードの作成・更新・削除をまとめて行う。
/// **注意**: AT Protocol仕様は本来これを単一commitで行うが、この実装は各要素を
/// `createRecord`/`putRecord`/`deleteRecord`と同じ経路で順番にコミットする
/// （＝要素ごとに別のcommitになる。途中で失敗すると、それより前の要素は既にコミット済みのまま
/// 残る＝部分適用される。トランザクション的な全体ロールバックは行わない）。
pub async fn xrpc_apply_writes(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<ApplyWritesRequest>,
) -> impl IntoResponse {
    for write in &req.writes {
        let collection = match write {
            ApplyWritesOp::Create { collection, .. } => collection,
            ApplyWritesOp::Update { collection, .. } => collection,
            ApplyWritesOp::Delete { collection, .. } => collection,
        };
        if let Some(resp) = reject_post_collection(collection) {
            return resp;
        }
    }

    let actor = match authenticate_atp_write(&state, &headers, &req.repo).await {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let at_did = actor
        .at_did
        .clone()
        .expect("authenticate_atp_writeで確認済み");

    let mut results = Vec::with_capacity(req.writes.len());
    for write in req.writes {
        let now = chrono::Utc::now();
        match write {
            ApplyWritesOp::Create {
                collection,
                rkey,
                value,
            } => {
                let rkey = rkey.unwrap_or_else(generate_tid);
                match state
                    .atp_service
                    .commit_generic_record(
                        actor.id,
                        collection.clone(),
                        rkey.clone(),
                        &value,
                        "create",
                        now,
                    )
                    .await
                {
                    Ok((_, record_cid)) => {
                        if collection == "app.bsky.actor.profile" {
                            sync_profile_to_actors(&state, &actor, &value).await;
                        }
                        results.push(serde_json::json!({
                            "$type": "com.atproto.repo.applyWrites#createResult",
                            "uri": format!("at://{}/{}/{}", at_did, collection, rkey),
                            "cid": cid_to_string(&record_cid),
                        }))
                    }
                    Err(e) => {
                        return ApiError::Internal(format!("[applyWrites] create失敗: {}", e))
                            .into_response()
                    }
                }
            }
            ApplyWritesOp::Update {
                collection,
                rkey,
                value,
            } => {
                match state
                    .atp_service
                    .commit_generic_record(
                        actor.id,
                        collection.clone(),
                        rkey.clone(),
                        &value,
                        "update",
                        now,
                    )
                    .await
                {
                    Ok((_, record_cid)) => {
                        if collection == "app.bsky.actor.profile" {
                            sync_profile_to_actors(&state, &actor, &value).await;
                        }
                        results.push(serde_json::json!({
                            "$type": "com.atproto.repo.applyWrites#updateResult",
                            "uri": format!("at://{}/{}/{}", at_did, collection, rkey),
                            "cid": cid_to_string(&record_cid),
                        }))
                    }
                    Err(e) => {
                        return ApiError::Internal(format!("[applyWrites] update失敗: {}", e))
                            .into_response()
                    }
                }
            }
            ApplyWritesOp::Delete { collection, rkey } => {
                match state
                    .atp_service
                    .delete_atp_record_generic(actor.id, &collection, &rkey, now)
                    .await
                {
                    Ok(()) => results.push(serde_json::json!({
                        "$type": "com.atproto.repo.applyWrites#deleteResult",
                    })),
                    Err(e) => {
                        return ApiError::Internal(format!("[applyWrites] delete失敗: {}", e))
                            .into_response()
                    }
                }
            }
        }
    }

    Json(serde_json::json!({ "results": results })).into_response()
}

/// DAG-CBOR デコード結果（`Ipld`）を AT Protocol の JSON 表現に変換する。
/// `serde_json::Value` へ直接デシリアライズすると、CID リンク（tag 42）を含む
/// レコード（embed の blob ref 等）で `invalid type: newtype struct` エラーになるため、
/// 一度 `Ipld` にデコードしてから AT Protocol の規約（CIDリンク→`{"$link": "<cid>"}`、
/// バイト列→`{"$bytes": "<base64>"}`）に沿って手動変換する。
fn ipld_to_json(ipld: &ipld_core::ipld::Ipld) -> serde_json::Value {
    use ipld_core::ipld::Ipld;
    match ipld {
        Ipld::Null => serde_json::Value::Null,
        Ipld::Bool(b) => serde_json::Value::Bool(*b),
        Ipld::Integer(i) => serde_json::json!(i),
        Ipld::Float(f) => serde_json::json!(f),
        Ipld::String(s) => serde_json::Value::String(s.clone()),
        Ipld::Bytes(b) => serde_json::json!({ "$bytes": URL_SAFE_NO_PAD.encode(b) }),
        Ipld::List(l) => serde_json::Value::Array(l.iter().map(ipld_to_json).collect()),
        Ipld::Map(m) => serde_json::Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), ipld_to_json(v)))
                .collect(),
        ),
        Ipld::Link(cid) => serde_json::json!({ "$link": cid.to_string() }),
    }
}

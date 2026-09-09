//! アカウント転入（既存DIDインポート）フロー専用の、移行元PDS（PDS A）向けXRPCクライアント。
//!
//! `atp/client.rs` はAppView閲覧＋自PDSへのpost系コミット用途で完結しているため、
//! 移行元は任意のハンドル（＝攻撃者が自由に指定できる入力）である転入フロー専用の
//! クライアントロジックはここに分離する。
//!
//! 全関数で `did_resolve::resolve_service_endpoint` が検証したIPへ接続を固定する
//! （SSRF対策、`client.rs::fetch_seiran_actor_declaration` と同じ型）。

use super::did_resolve::{resolve_service_endpoint, DidResolveError, ResolvedServiceEndpoint};
use super::handle_resolve::resolve_external_handle;
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum MigrationClientError {
    #[error("ハンドルの解決に失敗しました: {0}")]
    HandleResolve(String),
    #[error("移行元PDSの解決に失敗しました: {0}")]
    ServiceResolve(#[from] DidResolveError),
    #[error("接続先の検証に失敗しました: {0}")]
    Resolve(String),
    #[error("HTTPエラー: {0}")]
    Http(String),
    #[error("レスポンスのパースに失敗しました: {0}")]
    Parse(String),
    /// `createSession` が `AuthFactorTokenRequired` を返した。PDS A登録メール宛に
    /// コードが送付済みのはずなので、呼び出し側はユーザーに入力を求めて
    /// `auth_factor_token` 付きで再試行する。
    #[error("PDS Aがメール確認コードを要求しています")]
    AuthFactorTokenRequired,
    #[error("XRPCエラー (HTTP {status}): {code} - {message}")]
    Xrpc {
        status: u16,
        code: String,
        message: String,
    },
}

fn xrpc_error_from_body(status: u16, body: &serde_json::Value) -> MigrationClientError {
    let code = body
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let message = body
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if code == "AuthFactorTokenRequired" {
        return MigrationClientError::AuthFactorTokenRequired;
    }
    MigrationClientError::Xrpc {
        status,
        code,
        message,
    }
}

/// `resolved.url` が指すホストへ接続を固定した `reqwest::Client` を構築する。
/// [SEC-3] `resolve_service_endpoint` が検証済みのIPへ`resolve_to_addrs`で固定することで、
/// 検証後の再解決によるDNS rebindingを防ぐ（`client.rs::fetch_seiran_actor_declaration`と同型）。
fn build_pinned_client(resolved: &ResolvedServiceEndpoint) -> Result<reqwest::Client, MigrationClientError> {
    let host = reqwest::Url::parse(&resolved.url)
        .map_err(|e| MigrationClientError::Resolve(e.to_string()))?
        .host_str()
        .ok_or_else(|| MigrationClientError::Resolve("ホスト名を取得できません".into()))?
        .to_string();
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        // getRepoはアカウント規模次第で大きくなりうるため長めに取る。
        .timeout(Duration::from_secs(120))
        .resolve_to_addrs(&host, &resolved.addresses)
        .build()
        .map_err(|e| MigrationClientError::Http(e.to_string()))
}

/// ハンドルから移行元PDS（PDS A）のDIDとサービスエンドポイントを解決する。
/// フロー最初のステップ（`POST /api/migration/start`）で使う。
pub async fn resolve_source_pds(
    handle: &str,
    http: &reqwest::Client,
) -> Result<(String, ResolvedServiceEndpoint), MigrationClientError> {
    let did = resolve_external_handle(handle, http)
        .await
        .ok_or_else(|| MigrationClientError::HandleResolve(format!("ハンドル {handle} を解決できません")))?;
    let resolved = resolve_service_endpoint(&did, "atproto_pds").await?;
    Ok((did, resolved))
}

#[derive(Debug, Clone)]
pub struct AtpSession {
    pub did: String,
    pub handle: String,
    pub access_jwt: String,
    pub refresh_jwt: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSessionResp {
    did: String,
    handle: String,
    access_jwt: String,
    refresh_jwt: String,
}

/// `com.atproto.server.createSession`。`auth_factor_token` はPDS Aがメール2FAを要求した
/// 場合の再試行時のみ指定する。
pub async fn create_session_with_2fa(
    resolved: &ResolvedServiceEndpoint,
    identifier: &str,
    password: &str,
    auth_factor_token: Option<&str>,
) -> Result<AtpSession, MigrationClientError> {
    let client = build_pinned_client(resolved)?;
    let mut body = serde_json::json!({
        "identifier": identifier,
        "password": password,
    });
    if let Some(token) = auth_factor_token {
        body["authFactorToken"] = serde_json::Value::String(token.to_string());
    }

    let resp = client
        .post(format!(
            "{}/xrpc/com.atproto.server.createSession",
            resolved.url
        ))
        .json(&body)
        .send()
        .await
        .map_err(|e| MigrationClientError::Http(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        let err_body: serde_json::Value = resp.json().await.unwrap_or_default();
        return Err(xrpc_error_from_body(status.as_u16(), &err_body));
    }

    let session: CreateSessionResp = resp
        .json()
        .await
        .map_err(|e| MigrationClientError::Parse(e.to_string()))?;
    Ok(AtpSession {
        did: session.did,
        handle: session.handle,
        access_jwt: session.access_jwt,
        refresh_jwt: session.refresh_jwt,
    })
}

/// `com.atproto.sync.getRepo` — リポジトリ全体をCAR(v1)バイト列で取得する。
pub async fn fetch_repo_car(
    resolved: &ResolvedServiceEndpoint,
    session: &AtpSession,
) -> Result<Vec<u8>, MigrationClientError> {
    let client = build_pinned_client(resolved)?;
    let url = format!(
        "{}/xrpc/com.atproto.sync.getRepo?did={}",
        resolved.url,
        urlencoding::encode(&session.did)
    );
    let mut req = client.get(&url);
    if !session.access_jwt.is_empty() {
        req = req.bearer_auth(&session.access_jwt);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| MigrationClientError::Http(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let err_body: serde_json::Value =
            serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
        return Err(xrpc_error_from_body(status.as_u16(), &err_body));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| MigrationClientError::Http(e.to_string()))
}

#[derive(Debug, Deserialize)]
struct ListBlobsResp {
    cids: Vec<String>,
    cursor: Option<String>,
}

/// `com.atproto.sync.listBlobs` — ページングして全blob CIDを収集する。
pub async fn list_blobs(
    resolved: &ResolvedServiceEndpoint,
    session: &AtpSession,
) -> Result<Vec<String>, MigrationClientError> {
    let client = build_pinned_client(resolved)?;
    let mut cids = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut url = format!(
            "{}/xrpc/com.atproto.sync.listBlobs?did={}&limit=1000",
            resolved.url,
            urlencoding::encode(&session.did)
        );
        if let Some(c) = &cursor {
            url.push_str(&format!("&cursor={}", urlencoding::encode(c)));
        }

        let mut req = client.get(&url);
        if !session.access_jwt.is_empty() {
            req = req.bearer_auth(&session.access_jwt);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| MigrationClientError::Http(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let err_body: serde_json::Value = resp.json().await.unwrap_or_default();
            return Err(xrpc_error_from_body(status.as_u16(), &err_body));
        }

        let page: ListBlobsResp = resp
            .json()
            .await
            .map_err(|e| MigrationClientError::Parse(e.to_string()))?;
        let got_any = !page.cids.is_empty();
        cids.extend(page.cids);

        match page.cursor {
            Some(next) if got_any => cursor = Some(next),
            _ => break,
        }
    }

    Ok(cids)
}

/// `com.atproto.sync.getBlob` — 1件のblobバイト列を取得する。
pub async fn fetch_blob(
    resolved: &ResolvedServiceEndpoint,
    session: &AtpSession,
    cid: &str,
) -> Result<Vec<u8>, MigrationClientError> {
    let client = build_pinned_client(resolved)?;
    let url = format!(
        "{}/xrpc/com.atproto.sync.getBlob?did={}&cid={}",
        resolved.url,
        urlencoding::encode(&session.did),
        urlencoding::encode(cid)
    );
    let mut req = client.get(&url);
    if !session.access_jwt.is_empty() {
        req = req.bearer_auth(&session.access_jwt);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| MigrationClientError::Http(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let err_body: serde_json::Value =
            serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
        return Err(xrpc_error_from_body(status.as_u16(), &err_body));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| MigrationClientError::Http(e.to_string()))
}

/// `com.atproto.identity.requestPlcOperationSignature` — PDS A登録メールへ確認コードを
/// 送付させる。成功レスポンスの中身は空（`{}`）。
pub async fn request_plc_operation_signature(
    resolved: &ResolvedServiceEndpoint,
    session: &AtpSession,
) -> Result<(), MigrationClientError> {
    let client = build_pinned_client(resolved)?;
    let resp = client
        .post(format!(
            "{}/xrpc/com.atproto.identity.requestPlcOperationSignature",
            resolved.url
        ))
        .bearer_auth(&session.access_jwt)
        .send()
        .await
        .map_err(|e| MigrationClientError::Http(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        let err_body: serde_json::Value = resp.json().await.unwrap_or_default();
        return Err(xrpc_error_from_body(status.as_u16(), &err_body));
    }
    Ok(())
}

/// `signPlcOperation` へ渡す新DIDドキュメントの内容。実装方針（Phase 0検証結果）どおり、
/// 省略に頼らず常にフルセットで渡す。
pub struct DesiredDidCredentials {
    pub rotation_keys: Vec<String>,
    pub also_known_as: Vec<String>,
    pub verification_methods: serde_json::Value,
    pub services: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct SignPlcOperationResp {
    operation: serde_json::Value,
}

/// `com.atproto.identity.signPlcOperation` — メールで届いたtokenと、seiranが希望する
/// 新DIDドキュメントの内容を渡し、PDS Aに（PDS Aが保持するrotation keyで）署名させる。
/// 戻り値は未提出の署名済みPLCオペレーション。
pub async fn sign_plc_operation(
    resolved: &ResolvedServiceEndpoint,
    session: &AtpSession,
    token: &str,
    desired: &DesiredDidCredentials,
) -> Result<serde_json::Value, MigrationClientError> {
    let client = build_pinned_client(resolved)?;
    let body = serde_json::json!({
        "token": token,
        "rotationKeys": desired.rotation_keys,
        "alsoKnownAs": desired.also_known_as,
        "verificationMethods": desired.verification_methods,
        "services": desired.services,
    });

    let resp = client
        .post(format!(
            "{}/xrpc/com.atproto.identity.signPlcOperation",
            resolved.url
        ))
        .bearer_auth(&session.access_jwt)
        .json(&body)
        .send()
        .await
        .map_err(|e| MigrationClientError::Http(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        let err_body: serde_json::Value = resp.json().await.unwrap_or_default();
        return Err(xrpc_error_from_body(status.as_u16(), &err_body));
    }

    let parsed: SignPlcOperationResp = resp
        .json()
        .await
        .map_err(|e| MigrationClientError::Parse(e.to_string()))?;
    Ok(parsed.operation)
}

/// `com.atproto.identity.submitPlcOperation` — 署名済みオペレーションをPLCディレクトリへ
/// 提出する。実体は`atp/plc.rs::submit_plc_operation`（`submit_plc_genesis`と同じ
/// エンドポイント `{plc_directory_base_url()}/{did}` を共有）。
/// ★不可逆境界: これが成功すると、DIDのservice endpointが実際にseiranへ切り替わる。
pub async fn submit_plc_operation(
    did: &str,
    operation: &serde_json::Value,
    http: &reqwest::Client,
) -> Result<(), super::plc::PlcError> {
    super::plc::submit_plc_operation_raw(did, operation, http).await
}

/// `com.atproto.server.deactivateAccount` — 旧アカウントの無効化（ベストエフォート）。
pub async fn deactivate_account(
    resolved: &ResolvedServiceEndpoint,
    session: &AtpSession,
) -> Result<(), MigrationClientError> {
    let client = build_pinned_client(resolved)?;
    let resp = client
        .post(format!(
            "{}/xrpc/com.atproto.server.deactivateAccount",
            resolved.url
        ))
        .bearer_auth(&session.access_jwt)
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| MigrationClientError::Http(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        let err_body: serde_json::Value = resp.json().await.unwrap_or_default();
        return Err(xrpc_error_from_body(status.as_u16(), &err_body));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `@testimport.bsky.social`（did:plc:n3tpur22sxb57zgzwe3lkl5m）に対する実機疎通確認。
    /// パスワードを要する`createSession`以降は含めず、認証不要な読み取り系のみを検証する
    /// （ハンドル解決→サービスエンドポイント解決→SSRF対策込みの実リクエスト、が
    /// 一連で動くことの確認が目的）。ネットワークアクセスを行うため通常のテスト実行では
    /// 走らせず、`cargo test -- --ignored` で明示的に実行する。
    #[tokio::test]
    #[ignore]
    async fn resolves_and_fetches_real_test_account() {
        let http = reqwest::Client::new();
        let (did, resolved) = resolve_source_pds("testimport.bsky.social", &http)
            .await
            .expect("ハンドル解決に失敗");
        assert_eq!(did, "did:plc:n3tpur22sxb57zgzwe3lkl5m");
        assert!(resolved.url.contains("host.bsky.network"));

        let dummy_session = AtpSession {
            did: did.clone(),
            handle: "testimport.bsky.social".to_string(),
            access_jwt: String::new(),
            refresh_jwt: String::new(),
        };

        let car = fetch_repo_car(&resolved, &dummy_session)
            .await
            .expect("getRepoに失敗");
        assert!(!car.is_empty());
        // Phase 1のCARデコーダで実際に読み切れることも合わせて確認する。
        let records = crate::atp::mst_walk::decode_repo_records(&car).expect("CARデコードに失敗");
        for r in &records {
            assert!(!r.collection.is_empty());
        }

        let blob_cids = list_blobs(&resolved, &dummy_session)
            .await
            .expect("listBlobsに失敗");
        // 空でもエラーでなければ良い（テストアカウントにblobが無い可能性がある）。
        let _ = blob_cids;
    }
}

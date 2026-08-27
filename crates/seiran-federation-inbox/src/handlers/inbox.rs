use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use base64::Engine as _;
use seiran_common::repository::{PgRelayRepository, Relay, RelayRepository, RelayStatus};
use seiran_common::traits::Job;
use sha2::{Digest as Sha2Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

use crate::AppState;

/// AP Inbox エンドポイント。
///
/// 署名検証（低レイテンシ必須）だけをここで同期的に行い、アクティビティの実処理は
/// すべて `Job::InboundActivityProcess` としてジョブキューへ委譲する
/// （実処理は `seiran_common::jobs::inbound_activity_process` → Worker が担う）。
pub async fn inbox_handler(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let header_map: HashMap<String, String> = headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|val| (k.as_str().to_lowercase(), val.to_string()))
        })
        .collect();

    // [HIGH-01-①] Digest ヘッダーの必須化とボディ完全性検証
    let body_hash = Sha256::digest(&body);
    let computed_digest = format!(
        "SHA-256={}",
        base64::prelude::BASE64_STANDARD.encode(body_hash)
    );
    match header_map.get("digest") {
        Some(received_digest) if received_digest == &computed_digest => {}
        Some(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                "Digest ヘッダーがボディと一致しません",
            )
                .into_response();
        }
        None => {
            return (StatusCode::UNAUTHORIZED, "Digest ヘッダーが必要です").into_response();
        }
    }

    let signature = match header_map.get("signature") {
        Some(s) => s.clone(),
        None => {
            return (StatusCode::UNAUTHORIZED, "署名ヘッダーが見つかりません").into_response();
        }
    };

    // Signature の headers= に "digest" が含まれることを確認
    if !signature_covers_digest(&signature) {
        return (
            StatusCode::UNAUTHORIZED,
            "Signature の headers= に digest が含まれていません",
        )
            .into_response();
    }

    let key_id = match extract_key_id(&signature) {
        Some(k) => k,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                "Signature に keyId が見つかりません",
            )
                .into_response();
        }
    };

    match state
        .ap_client
        .verify_signature("POST", "/inbox", &header_map, &signature)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return (StatusCode::UNAUTHORIZED, "署名検証失敗").into_response();
        }
        Err(e) => {
            tracing::error!("[Inbox] 署名検証エラー (keyId={}): {}", key_id, e);
            return (StatusCode::UNAUTHORIZED, format!("署名エラー: {}", e)).into_response();
        }
    }

    let raw_activity = String::from_utf8_lossy(&body).to_string();
    tracing::info!("[Inbox] アクティビティ受信 ({} bytes)", raw_activity.len());

    let activity: serde_json::Value = match serde_json::from_str(&raw_activity) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[Inbox] JSON パースエラー: {}", e);
            return (StatusCode::BAD_REQUEST, "JSON パースエラー").into_response();
        }
    };

    // [HIGH-01-②] keyId のアクター URI とアクティビティの actor フィールドの一致検証。
    // リレーは元投稿者を activity.actor に保ったまま、リレー自身の鍵で配送するため、
    // 登録済みリレーと署名者が同一originの場合だけ不一致を許可する。
    let key_actor_base = key_id.split('#').next().unwrap_or(&key_id);
    let activity_actor = activity["actor"].as_str().unwrap_or("");
    if key_actor_base != activity_actor {
        let relay = match registered_relay_for_signer(&state, key_actor_base).await {
            Ok(Some(relay)) => relay,
            Ok(None) => {
                tracing::info!(
                    "[Inbox] keyId のアクター ({}) と activity.actor ({}) が一致しません",
                    key_actor_base,
                    activity_actor
                );
                return (StatusCode::UNAUTHORIZED, "署名者とアクターが一致しません")
                    .into_response();
            }
            Err(e) => {
                tracing::error!("[Inbox] 登録リレー照合エラー: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "登録リレー照合エラー").into_response();
            }
        };

        // 一部のリレー実装はAcceptを返さず、Follow直後から配送を始める。
        // 正しい署名付き配送を受信できた時点で参加成立とみなす。
        if relay.status == RelayStatus::Pending {
            let relays = PgRelayRepository::new(state.db.clone());
            if let Err(e) = relays
                .update_status(relay.id, RelayStatus::Accepted, None)
                .await
            {
                tracing::error!("[Inbox] リレー(id={}) accepted更新エラー: {}", relay.id, e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "リレー状態更新エラー").into_response();
            }
            tracing::info!(
                "[Inbox] リレー(id={}) の署名付き配送を受信しacceptedへ更新",
                relay.id
            );
        }
    }

    let activity_type = activity["type"].as_str().unwrap_or("(不明)").to_string();
    if let Err(e) = state
        .job_queue
        .enqueue(Job::InboundActivityProcess { raw_activity }, 10)
        .await
    {
        tracing::error!("[Inbox] type={} のエンキュー失敗: {}", activity_type, e);
    }

    (StatusCode::ACCEPTED, "").into_response()
}

async fn registered_relay_for_signer(
    state: &AppState,
    signer_actor: &str,
) -> Result<Option<Relay>, seiran_common::repository::RelayError> {
    let relays = PgRelayRepository::new(state.db.clone());
    Ok(relays
        .list_all()
        .await?
        .into_iter()
        .find(|relay| urls_have_same_origin(signer_actor, &relay.inbox_url)))
}

fn urls_have_same_origin(left: &str, right: &str) -> bool {
    match (reqwest::Url::parse(left), reqwest::Url::parse(right)) {
        (Ok(left), Ok(right)) => left.origin() == right.origin(),
        _ => false,
    }
}

/// Signature ヘッダーの headers= フィールドに "digest" が含まれているか確認する
fn signature_covers_digest(signature_header: &str) -> bool {
    for part in signature_header.split(',') {
        let kv: Vec<&str> = part.splitn(2, '=').collect();
        if kv.len() == 2 && kv[0].trim() == "headers" {
            let headers_val = kv[1].trim().trim_matches('"');
            return headers_val
                .split(' ')
                .any(|h| h.eq_ignore_ascii_case("digest"));
        }
    }
    false
}

/// Signature ヘッダーから keyId の値を抽出する
fn extract_key_id(signature_header: &str) -> Option<String> {
    for part in signature_header.split(',') {
        let kv: Vec<&str> = part.splitn(2, '=').collect();
        if kv.len() == 2 && kv[0].trim() == "keyId" {
            return Some(kv[1].trim().trim_matches('"').to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{extract_key_id, signature_covers_digest, urls_have_same_origin};

    #[test]
    fn signature_covers_digest_with_digest() {
        let sig = r#"keyId="https://example.com/users/alice#main-key",algorithm="rsa-sha256",headers="(request-target) host date digest",signature="abc""#;
        assert!(signature_covers_digest(sig));
    }

    #[test]
    fn signature_covers_digest_without_digest() {
        let sig = r#"keyId="https://example.com/users/alice#main-key",algorithm="rsa-sha256",headers="(request-target) host date",signature="abc""#;
        assert!(!signature_covers_digest(sig));
    }

    #[test]
    fn signature_covers_digest_no_headers_field() {
        let sig = r#"keyId="https://example.com/users/alice#main-key",signature="abc""#;
        assert!(!signature_covers_digest(sig));
    }

    #[test]
    fn extract_key_id_basic() {
        let sig = r#"keyId="https://example.com/users/alice#main-key",algorithm="rsa-sha256",headers="(request-target) host date digest",signature="abc""#;
        assert_eq!(
            extract_key_id(sig),
            Some("https://example.com/users/alice#main-key".to_string())
        );
    }

    #[test]
    fn extract_key_id_missing() {
        let sig = r#"algorithm="rsa-sha256",signature="abc""#;
        assert_eq!(extract_key_id(sig), None);
    }

    #[test]
    fn relay_signer_origin_matches_registered_inbox() {
        assert!(urls_have_same_origin(
            "https://relay.example/actor",
            "https://relay.example/inbox"
        ));
        assert!(!urls_have_same_origin(
            "https://attacker.example/actor",
            "https://relay.example/inbox"
        ));
    }
}

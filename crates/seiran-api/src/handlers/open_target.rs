//! URL・AT URI・ユーザーIDを取り込み、SPA内の遷移先へ変換する。

use axum::{extract::State, Json};
use seiran_common::{job_priority, Job};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::handlers::target_resolve::resolve_and_upsert_target;
use crate::middleware::AuthedUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct OpenTargetRequest {
    pub target: String,
}

#[derive(Serialize)]
pub struct OpenTargetResponse {
    pub path: String,
    pub kind: &'static str,
}

enum ParsedTarget {
    BskyPost(String),
    Actor(String),
    ActivityPubUrl(String),
}

pub async fn open_target(
    State(state): State<AppState>,
    _user: AuthedUser,
    Json(req): Json<OpenTargetRequest>,
) -> Result<Json<OpenTargetResponse>, ApiError> {
    let parsed = parse_target(&req.target)
        .ok_or_else(|| ApiError::BadRequest("INVALID_OPEN_TARGET".to_string()))?;

    let response = match parsed {
        ParsedTarget::BskyPost(at_uri) => open_bsky_post(&state, &at_uri).await?,
        ParsedTarget::Actor(target) => open_actor(&state, &target).await?,
        ParsedTarget::ActivityPubUrl(url) => open_activitypub_url(&state, &url).await?,
    };
    Ok(Json(response))
}

fn parse_target(raw: &str) -> Option<ParsedTarget> {
    let target = raw.trim();
    if target.starts_with("at://") {
        return parse_at_post_uri(target).then(|| ParsedTarget::BskyPost(target.to_string()));
    }
    if target.starts_with("did:plc:") {
        return Some(ParsedTarget::Actor(target.to_string()));
    }
    if target.starts_with('@') {
        let acct = target.trim_start_matches('@');
        return (!acct.is_empty()).then(|| ParsedTarget::Actor(target.to_string()));
    }

    let url = url::Url::parse(target).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.host_str() == Some("bsky.app") {
        let parts: Vec<_> = url
            .path_segments()?
            .filter(|part| !part.is_empty())
            .collect();
        return match parts.as_slice() {
            ["profile", actor, "post", rkey] => Some(ParsedTarget::BskyPost(format!(
                "at://{actor}/app.bsky.feed.post/{rkey}"
            ))),
            ["profile", actor] => Some(ParsedTarget::Actor((*actor).to_string())),
            _ => None,
        };
    }
    Some(ParsedTarget::ActivityPubUrl(target.to_string()))
}

fn parse_at_post_uri(target: &str) -> bool {
    let parts: Vec<_> = target
        .trim_start_matches("at://")
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    matches!(parts.as_slice(), [_, "app.bsky.feed.post", _])
}

async fn open_actor(state: &AppState, target: &str) -> Result<OpenTargetResponse, ApiError> {
    let actor = resolve_and_upsert_target(state, target)
        .await
        .map_err(|_| ApiError::BadRequest("INVALID_OPEN_TARGET".to_string()))?;
    let acct = if actor.actor_type == "local" {
        format!("@{}", actor.username)
    } else {
        format!("@{}@{}", actor.username, actor.domain)
    };
    Ok(OpenTargetResponse {
        path: format!("/{}", acct),
        kind: "actor",
    })
}

async fn open_bsky_post(state: &AppState, at_uri: &str) -> Result<OpenTargetResponse, ApiError> {
    // bsky.app URLではハンドルがauthorityになる場合があるため、プロフィール取得でDIDへ正規化する。
    let parts: Vec<_> = at_uri.trim_start_matches("at://").split('/').collect();
    let profile = seiran_common::atp::fetch_bsky_profile(&state.http_client, parts[0])
        .await
        .map_err(|_| ApiError::BadRequest("INVALID_OPEN_TARGET".to_string()))?;
    let canonical_uri = format!("at://{}/app.bsky.feed.post/{}", profile.did, parts[2]);
    let post = seiran_common::atp::fetch_single_bsky_post(&state.http_client, &canonical_uri)
        .await
        .map_err(ApiError::BadGateway)?
        .ok_or_else(|| ApiError::BadRequest("INVALID_OPEN_TARGET".to_string()))?;
    let actor = resolve_and_upsert_target(state, &post.author_did)
        .await
        .map_err(|_| ApiError::BadRequest("INVALID_OPEN_TARGET".to_string()))?;
    let post_id = seiran_common::atp::upsert_bsky_post(&state.db, &state.job_queue, actor.id, &post)
        .await
        .map_err(|e| ApiError::Internal(format!("Bsky post保存失敗: {e}")))?;
    Ok(OpenTargetResponse {
        path: format!("/notes/{post_id}"),
        kind: "post",
    })
}

async fn open_activitypub_url(state: &AppState, url: &str) -> Result<OpenTargetResponse, ApiError> {
    if let Some(post_id) = state
        .posts
        .find_id_by_ap_or_at_uri(url)
        .await
        .map_err(|e| ApiError::Internal(format!("投稿検索失敗: {e}")))?
    {
        return Ok(OpenTargetResponse {
            path: format!("/notes/{post_id}"),
            kind: "post",
        });
    }

    let (body, _) = crate::handlers::media_proxy::fetch_validated_with_accept(
        url,
        &[
            "application/activity+json",
            "application/ld+json",
            "application/json",
        ],
        "application/activity+json, application/ld+json",
    )
    .await
    .map_err(|_| ApiError::BadRequest("INVALID_OPEN_TARGET".to_string()))?;
    let object: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::BadRequest("INVALID_OPEN_TARGET".to_string()))?;
    let object_type = object["type"].as_str().unwrap_or("");
    if matches!(
        object_type,
        "Person" | "Service" | "Application" | "Organization" | "Group"
    ) {
        return open_actor(state, object["id"].as_str().unwrap_or(url)).await;
    }
    // Misskeyの素リノート（コメント無しブースト）は、notes URLへの直接アクセスや他鯖ミラー
    // URLからの302リダイレクトの結果として`Announce`（`object`は対象ノートのURI文字列）に
    // 行き着く。通常投稿としてではなく正しくリポストラッパーとして取り込む（#232）。
    if object_type == "Announce" {
        return open_announce(state, url, object).await;
    }
    if !matches!(object_type, "Note" | "Article" | "Question" | "Page") {
        return Err(ApiError::BadRequest("INVALID_OPEN_TARGET".to_string()));
    }

    let note_id = object["id"].as_str().unwrap_or(url);
    let actor = object["attributedTo"]
        .as_str()
        .or_else(|| object["attributedTo"].as_array()?.first()?.as_str())
        .ok_or_else(|| ApiError::BadRequest("INVALID_OPEN_TARGET".to_string()))?;
    let activity = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": format!("{note_id}#seiran-open"),
        "type": "Create",
        "actor": actor,
        "object": object,
    });
    enqueue_and_await_import(state, activity, note_id).await
}

/// フェッチしたAnnounce（Misskeyの素リノート・他鯖ミラー経由でのAnnounce解決を含む）を
/// リポストラッパーとして取り込む。既存のCreate用合成ラップとは異なり、フェッチしたAnnounce
/// オブジェクト自体が`handle_announce`の期待する形（`id`/`actor`/`object`/`to`/`cc`/`published`）
/// を満たすため、そのまま`InboundActivityProcess`へ積む（#232）。対象ポスト（`object`）が
/// 未取得なら`resolve_reference`が1段階だけフェッチする（#231）。対象の取得に失敗しても
/// リポストの箱自体は保存されるため、ここでの完了待ちは箱の保存だけを待てば良い。
async fn open_announce(
    state: &AppState,
    url: &str,
    announce: serde_json::Value,
) -> Result<OpenTargetResponse, ApiError> {
    let announce_id = announce["id"].as_str().unwrap_or(url).to_string();
    if announce["actor"].as_str().is_none() {
        return Err(ApiError::BadRequest("INVALID_OPEN_TARGET".to_string()));
    }
    if announce["object"].as_str().is_none() {
        return Err(ApiError::BadRequest("INVALID_OPEN_TARGET".to_string()));
    }
    enqueue_and_await_import(state, announce, &announce_id).await
}

/// `Job::InboundActivityProcess`へ積み、`dedup_uri`（`ap_object_id`として保存されるはずの
/// URI）で該当投稿が保存されるまで短時間だけポーリングする。Note（Create経由）・
/// Announce（リポスト経由）の両方の「開く」経路で共有する。
async fn enqueue_and_await_import(
    state: &AppState,
    activity: serde_json::Value,
    dedup_uri: &str,
) -> Result<OpenTargetResponse, ApiError> {
    state
        .job_queue
        .enqueue(
            Job::InboundActivityProcess {
                raw_activity: activity.to_string(),
            },
            job_priority::HIGH,
        )
        .await
        .map_err(|e| ApiError::Internal(format!("投稿取り込みキュー投入失敗: {e}")))?;

    // インバウンド処理は既存のCreate/Announce経路を再利用する。短時間だけ完了を待ち、詳細画面へ確実に遷移する。
    for _ in 0..40 {
        if let Some(post_id) = state
            .posts
            .find_id_by_ap_or_at_uri(dedup_uri)
            .await
            .map_err(|e| ApiError::Internal(format!("投稿検索失敗: {e}")))?
        {
            return Ok(OpenTargetResponse {
                path: format!("/notes/{post_id}"),
                kind: "post",
            });
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(ApiError::ServiceUnavailable("OPEN_TARGET_IMPORT_PENDING"))
}

#[cfg(test)]
mod tests {
    use super::{parse_target, ParsedTarget};

    #[test]
    fn parses_bsky_post_url() {
        let ParsedTarget::BskyPost(uri) =
            parse_target("https://bsky.app/profile/alice.test/post/3abc").unwrap()
        else {
            panic!("post expected");
        };
        assert_eq!(uri, "at://alice.test/app.bsky.feed.post/3abc");
    }

    #[test]
    fn rejects_unrelated_bsky_url_and_non_post_at_uri() {
        assert!(parse_target("https://bsky.app/settings").is_none());
        assert!(parse_target("at://did:plc:x/app.bsky.actor.profile/self").is_none());
    }
}

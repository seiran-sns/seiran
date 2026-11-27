use std::time::Duration;

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::{Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect},
};
use serde::Deserialize;
use tokio::sync::broadcast;

use seiran_common::atp::mst_walk::{commit_data_root, mst_path_step, MstPathStep};
use seiran_common::atp::{
    build_commit_frame, build_identity_frame, cid_from_str, encode_car, Cid, CommitEvtOp,
};
use seiran_common::repository::actor::Actor;

use super::xrpc_error;
use crate::error::ApiError;
use crate::AppState;

#[derive(Deserialize)]
pub struct GetBlobParams {
    pub did: Option<String>,
    pub cid: String,
}

/// `com.atproto.sync.getBlob` — CID から Blob（画像バイナリ）を返す。
/// seiran はメディアを外部ストレージ (R2/S3) に保存しているため、CDN URL にリダイレクトする。
pub async fn xrpc_get_blob(
    Query(params): Query<GetBlobParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let cid = match params.cid.parse::<seiran_common::atp::Cid>() {
        Ok(c) => c,
        Err(_) => return ApiError::BadRequest("Invalid CID".to_string()).into_response(),
    };

    // CIDv1 raw (0x55) または dag-cbor (0x71) の sha2-256 multihash からハッシュを取得
    let mh = cid.hash();
    if mh.code() != 0x12 {
        return ApiError::BadRequest("Unsupported hash function (expected sha2-256)".to_string())
            .into_response();
    }
    let sha256_hex = hex::encode(mh.digest());

    // media_files（Seiran自前UI経由・com.atproto.repo.uploadBlob 経由〈Bsky公式動画
    // パイプラインの代理POSTを含む〉のどちらでアップロードされたものも入っている）を検索する。
    let row =
        seiran_common::repository::media_file::public_location_by_sha256(&state.db, &sha256_hex)
            .await;

    match row {
        Ok(Some((_mime_type, url))) => {
            if url.is_empty() {
                return ApiError::NotFound("Blob not found").into_response();
            }
            Redirect::temporary(&url).into_response()
        }
        Ok(None) => {
            match fallback_avatar_blob_response(&state, params.did.as_deref(), &sha256_hex).await {
                Some(resp) => resp,
                None => ApiError::NotFound("Blob not found").into_response(),
            }
        }
        Err(e) => ApiError::Internal(format!("[getBlob] DB エラー: {}", e)).into_response(),
    }
}

/// `did` が未設定アバターのローカル actor を指し、要求された CID が
/// `fallback_avatar_atp_blob` の決定論的生成結果と一致する場合のみ、その場で PNG を
/// 生成して返す。ストレージには一切保存しない（`avatar_media_id` が無いローカル actor は
/// 常に同じ入力から同じ CID になるため、bsky.app からは実在する blob として安定して見える）。
async fn fallback_avatar_blob_response(
    state: &AppState,
    did: Option<&str>,
    requested_sha256_hex: &str,
) -> Option<axum::response::Response> {
    let did = did?;
    let actor = state.actors.find_by_did(did).await.ok()??;
    if actor.actor_type != "local" {
        return None;
    }
    let (sha256_hex, mime, bytes_len) = seiran_common::avatar::fallback_avatar_atp_blob(actor.id);
    if sha256_hex != requested_sha256_hex {
        return None;
    }
    let bytes = seiran_common::avatar::fallback_avatar_png(actor.id);
    debug_assert_eq!(bytes.len() as i64, bytes_len);
    Some(
        (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(mime)),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000, immutable"),
                ),
            ],
            bytes,
        )
            .into_response(),
    )
}

#[derive(Deserialize)]
pub struct GetRepoParams {
    pub did: String,
}

#[derive(Deserialize)]
pub struct SubscribeReposParams {
    pub cursor: Option<i64>,
}

pub async fn xrpc_get_repo(
    Query(params): Query<GetRepoParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor = match state.actors.find_by_did(&params.did).await {
        Ok(Some(a)) => a,
        Ok(None) => return ApiError::NotFound("DID が見つかりません").into_response(),
        Err(e) => {
            return ApiError::Internal(format!("[getRepo] アクター取得失敗: {}", e))
                .into_response();
        }
    };

    let actor_id = actor.id;
    let commit_cid_str = match actor.at_repo_cid {
        Some(s) => s,
        None => return ApiError::NotFound("リポジトリが未初期化です").into_response(),
    };

    let commit_cid = match cid_from_str(&commit_cid_str) {
        Ok(c) => c,
        Err(_) => return ApiError::Internal("commit CID パース失敗".to_string()).into_response(),
    };

    let block_rows = match state.atp_repo.find_blocks_by_actor(actor_id).await {
        Ok(r) => r,
        Err(e) => {
            return ApiError::Internal(format!("[getRepo] ブロック取得失敗: {}", e))
                .into_response();
        }
    };

    let blocks: Vec<_> = block_rows
        .into_iter()
        .filter_map(|(cid_str, bytes)| {
            let cid = cid_from_str(&cid_str).ok()?;
            Some((cid, bytes))
        })
        .collect();

    match encode_car(&commit_cid, &blocks) {
        Ok(car_bytes) => (
            StatusCode::OK,
            [("Content-Type", "application/vnd.ipld.car")],
            car_bytes,
        )
            .into_response(),
        Err(e) => ApiError::Internal(format!("[getRepo] CAR 生成失敗: {}", e)).into_response(),
    }
}

#[derive(Deserialize)]
pub struct ListReposParams {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

/// `com.atproto.sync.listRepos` — この PDS が保持する全リポジトリ（＝ATPアカウントを持つ
/// アクター）を id 順にページングして返す。クローラー・インデクサーが PDS 上の全アカウントを
/// 発見するために使う標準エンドポイント。
pub async fn xrpc_list_repos(
    Query(params): Query<ListReposParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(500).clamp(1, 1000);
    let cursor_id = params.cursor.as_deref().and_then(|c| c.parse::<i64>().ok());

    let actors = match state.actors.list_atp_repos(limit, cursor_id).await {
        Ok(a) => a,
        Err(e) => {
            return ApiError::Internal(format!("[listRepos] DB エラー: {}", e)).into_response();
        }
    };

    let next_cursor = actors.last().map(|a| a.id.to_string());
    let repos: Vec<_> = actors
        .into_iter()
        .filter_map(|a| {
            let did = a.at_did?;
            Some(serde_json::json!({
                "did": did,
                "head": a.at_repo_cid,
                "rev": a.at_repo_rev,
                "active": true,
            }))
        })
        .collect();

    axum::Json(serde_json::json!({
        "cursor": next_cursor,
        "repos": repos,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct GetLatestCommitParams {
    pub did: String,
}

/// `com.atproto.sync.getLatestCommit` — 指定 DID の現在の commit CID / rev を返す。
pub async fn xrpc_get_latest_commit(
    Query(params): Query<GetLatestCommitParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor = match state.actors.find_by_did(&params.did).await {
        Ok(Some(a)) => a,
        Ok(None) => return ApiError::NotFound("DID が見つかりません").into_response(),
        Err(e) => {
            return ApiError::Internal(format!("[getLatestCommit] アクター取得失敗: {}", e))
                .into_response();
        }
    };
    let cid = match actor.at_repo_cid {
        Some(c) => c,
        None => return ApiError::NotFound("リポジトリが未初期化です").into_response(),
    };
    let rev = actor.at_repo_rev.unwrap_or_default();

    axum::Json(serde_json::json!({ "cid": cid, "rev": rev })).into_response()
}

#[derive(Deserialize)]
pub struct GetRepoStatusParams {
    pub did: String,
}

/// アクターの状態を `com.atproto.sync.getRepoStatus` の `active` / `status` に写す。
/// firehose の `#account` で流している値（退会=`deleted`、DID転出=`deactivated`）と揃える。
/// 凍結は `#account` を流していない（解除しても active に戻ったと伝える手段が無い）ので、
/// ここでも active として返す。
fn repo_status(withdrawn: bool, did_moved_out: bool) -> (bool, Option<&'static str>) {
    if withdrawn {
        (false, Some("deleted"))
    } else if did_moved_out {
        (false, Some("deactivated"))
    } else {
        (true, None)
    }
}

/// この PDS がリポジトリを持つローカルアクター。ローカルのリポジトリを持たない DID
/// （リモートアクター等）は XRPC 標準どおり `RepoNotFound`。
async fn find_local_repo_actor(
    state: &AppState,
    did: &str,
    method: &str,
) -> Result<Actor, axum::response::Response> {
    match state.actors.find_by_did(did).await {
        Ok(Some(a)) if a.user_id.is_some() && a.at_repo_cid.is_some() => Ok(a),
        Ok(_) => Err(xrpc_error(
            "RepoNotFound",
            format!("Could not find repo for DID: {did}"),
        )),
        Err(e) => {
            Err(ApiError::Internal(format!("[{method}] アクター取得失敗: {}", e)).into_response())
        }
    }
}

/// `com.atproto.sync.getRepoStatus` — この PDS が持つリポジトリの稼働状態を返す。
/// Relay などが `#account` を取りこぼしたときの確認に使う。
pub async fn xrpc_get_repo_status(
    Query(params): Query<GetRepoStatusParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor = match find_local_repo_actor(&state, &params.did, "getRepoStatus").await {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let (active, status) = repo_status(
        actor.withdrawn_at.is_some(),
        actor.did_moved_out_at.is_some(),
    );
    let mut body = serde_json::json!({ "did": params.did, "active": active });
    if let Some(status) = status {
        body["status"] = status.into();
    }
    if active {
        if let Some(rev) = actor.at_repo_rev {
            body["rev"] = rev.into();
        }
    }
    axum::Json(body).into_response()
}

#[derive(Deserialize)]
pub struct GetRecordParams {
    pub did: String,
    pub collection: String,
    pub rkey: String,
}

/// 中身を返せないアカウントの状態を XRPC のエラー名に写す（参照実装の
/// `assertRepoAvailability` と同じ名前）。退会は参照実装ではアカウントごと消えるので
/// `RepoNotFound`、凍結は `RepoTakendown`、DID 転出は `RepoDeactivated`。
fn unavailable_repo_error(actor: &Actor) -> Option<&'static str> {
    if actor.withdrawn_at.is_some() {
        Some("RepoNotFound")
    } else if actor.suspended_at.is_some() {
        Some("RepoTakendown")
    } else if actor.did_moved_out_at.is_some() {
        Some("RepoDeactivated")
    } else {
        None
    }
}

/// `com.atproto.sync.getRecord` — レコード1件を、署名付きコミットからそのレコードまでの
/// MST の経路のブロックと一緒に CAR で返す（root はコミットの CID）。取得した側は DID の
/// 署名鍵でコミットを検証し、経路を辿ってレコードの正しさを確かめられる。レコードが無い
/// ときも、無いことを示す経路（コミットと、キーが入るはずの部分木までのノード）を返す。
pub async fn xrpc_get_record(
    Query(params): Query<GetRecordParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor = match find_local_repo_actor(&state, &params.did, "sync.getRecord").await {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if let Some(error) = unavailable_repo_error(&actor) {
        let message = match error {
            "RepoNotFound" => format!("Could not find repo for DID: {}", params.did),
            "RepoTakendown" => format!("Repo has been takendown: {}", params.did),
            _ => format!("Repo has been deactivated: {}", params.did),
        };
        return xrpc_error(error, message);
    }
    let key = format!("{}/{}", params.collection, params.rkey);
    let commit_cid_str = actor
        .at_repo_cid
        .as_deref()
        .expect("find_local_repo_actor で確認済み");
    match record_proof_blocks(&state, actor.id, commit_cid_str, &key).await {
        Ok((commit_cid, blocks)) => match encode_car(&commit_cid, &blocks) {
            Ok(car_bytes) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/vnd.ipld.car")],
                car_bytes,
            )
                .into_response(),
            Err(e) => {
                ApiError::Internal(format!("[sync.getRecord] CAR 生成失敗: {}", e)).into_response()
            }
        },
        Err(e) => ApiError::Internal(format!("[sync.getRecord] {}", e)).into_response(),
    }
}

/// コミット・`key` までの MST のノード・（あれば）レコード本体のブロックを、この順で集める。
/// ノードは1段ずつ取得するので、読むブロックは木の深さ程度で済む。
async fn record_proof_blocks(
    state: &AppState,
    actor_id: i64,
    commit_cid_str: &str,
    key: &str,
) -> Result<(Cid, Vec<(Cid, Vec<u8>)>), String> {
    use seiran_common::atp::car::MAX_MST_DEPTH;
    let block = |cid: Cid| async move {
        seiran_common::repository::atp::block_bytes(
            &state.db,
            actor_id,
            &seiran_common::atp::cid_to_string(&cid),
        )
        .await
        .map_err(|e| format!("ブロック取得失敗 {cid}: {e}"))?
        .ok_or_else(|| format!("ブロックがありません {cid}"))
    };

    let commit_cid =
        cid_from_str(commit_cid_str).map_err(|e| format!("commit CID パース失敗: {e}"))?;
    let commit_bytes = block(commit_cid).await?;
    let mut node_cid = commit_data_root(&commit_bytes).map_err(|e| e.to_string())?;
    let mut blocks = vec![(commit_cid, commit_bytes)];

    for _ in 0..=MAX_MST_DEPTH {
        let node_bytes = block(node_cid).await?;
        let step = mst_path_step(&node_bytes, key).map_err(|e| e.to_string())?;
        blocks.push((node_cid, node_bytes));
        match step {
            MstPathStep::Descend(child) => node_cid = child,
            MstPathStep::Found(record_cid) => {
                blocks.push((record_cid, block(record_cid).await?));
                return Ok((commit_cid, blocks));
            }
            MstPathStep::Absent => return Ok((commit_cid, blocks)),
        }
    }
    Err(format!("MST が深すぎます（{MAX_MST_DEPTH} 段超）"))
}

#[derive(Deserialize)]
pub struct ListBlobsParams {
    pub did: String,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

/// `com.atproto.sync.listBlobs` — 指定 DID がアップロードした blob の CID 一覧をページングして返す。
pub async fn xrpc_list_blobs(
    Query(params): Query<ListBlobsParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor = match state.actors.find_by_did(&params.did).await {
        Ok(Some(a)) => a,
        Ok(None) => return ApiError::NotFound("DID が見つかりません").into_response(),
        Err(e) => {
            return ApiError::Internal(format!("[listBlobs] アクター取得失敗: {}", e))
                .into_response();
        }
    };
    let limit = params.limit.unwrap_or(500).clamp(1, 1000);
    let cursor_id = params.cursor.as_deref().and_then(|c| c.parse::<i64>().ok());

    let rows = match state
        .atp_repo
        .list_blob_cids(actor.id, limit, cursor_id)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return ApiError::Internal(format!("[listBlobs] DB エラー: {}", e)).into_response();
        }
    };

    let next_cursor = rows.last().map(|(id, _)| id.to_string());
    let cids: Vec<String> = rows.into_iter().map(|(_, cid)| cid).collect();

    axum::Json(serde_json::json!({
        "cursor": next_cursor,
        "cids": cids,
    }))
    .into_response()
}

pub async fn xrpc_subscribe_repos(
    ws: WebSocketUpgrade,
    Query(params): Query<SubscribeReposParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_subscribe_repos(socket, params, state))
}

async fn handle_subscribe_repos(
    mut socket: WebSocket,
    params: SubscribeReposParams,
    state: AppState,
) {
    let mut rx = state.atp_service.events().subscribe();

    if let Some(mut cursor) = params.cursor {
        const BACKFILL_PAGE_SIZE: i64 = 500;
        loop {
            let events = state
                .atp_repo
                .find_events_after(cursor, BACKFILL_PAGE_SIZE)
                .await
                .unwrap_or_default();
            let page_len = events.len() as i64;

            for evt in events {
                cursor = evt.id;

                // frame_bytes が保存済みなら解凍してそのまま送る（再構築なし）
                if let Some(ref compressed) = evt.frame_bytes {
                    match zstd::decode_all(&compressed[..]) {
                        Ok(frame) => {
                            if socket.send(Message::Binary(frame)).await.is_err() {
                                return;
                            }
                            continue;
                        }
                        Err(e) => {
                            tracing::error!(
                                "[subscribeRepos] frame_bytes 解凍失敗 id={}: {}",
                                evt.id,
                                e
                            );
                        }
                    }
                }

                // frame_bytes が NULL の旧レコードは event_type に応じて再構築する
                let time_str = evt
                    .created_at
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                let frame_result = if evt.event_type == "identity" {
                    let handle = evt.handle.as_deref().unwrap_or("");
                    build_identity_frame(evt.id, &evt.did, handle, &time_str)
                } else {
                    let commit_cid =
                        match evt.commit_cid.as_deref().and_then(|s| cid_from_str(s).ok()) {
                            Some(c) => c,
                            None => continue,
                        };
                    let prev_cid = evt.prev_cid.as_deref().and_then(|s| cid_from_str(s).ok());
                    let ops: Vec<CommitEvtOp> = evt
                        .ops_json
                        .as_ref()
                        .and_then(|j| j.as_array())
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|op| {
                            let action = op["action"].as_str()?.to_string();
                            let path = op["path"].as_str()?.to_string();
                            let cid = op["cid"].as_str().and_then(|s| cid_from_str(s).ok());
                            Some(CommitEvtOp { action, path, cid })
                        })
                        .collect();
                    let car = evt.car_bytes.as_deref().unwrap_or(&[]);
                    let rev = evt.rev.as_deref().unwrap_or("");
                    // frame_bytes 未保存の旧イベント再構築用フォールバック。atp_repo_events は
                    // コミット時点の prevData を保持していないため None を渡す（通常この経路は
                    // 使われない。新規コミットは commit_record_inner 側で frame_bytes に
                    // prevData 込みで保存済み）。
                    build_commit_frame(&seiran_common::atp::repo::CommitFrame {
                        seq: evt.id,
                        did: &evt.did,
                        commit_cid: &commit_cid,
                        prev_cid: prev_cid.as_ref(),
                        rev,
                        since: evt.since_rev.as_deref(),
                        car_bytes: car,
                        ops: &ops,
                        blob_cids: &[],
                        time: &time_str,
                        prev_data: None,
                    })
                };
                if let Ok(frame) = frame_result {
                    if socket.send(Message::Binary(frame)).await.is_err() {
                        return;
                    }
                }
            }

            // 取得件数がページサイズ未満なら残りは無い。ページ丁度なら続きがある
            // 可能性があるため、最後に送った id を新しい cursor として再取得する。
            if page_len < BACKFILL_PAGE_SIZE {
                break;
            }
        }
    }

    let mut ping_interval = tokio::time::interval(Duration::from_secs(30));
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = state.shutdown.cancelled() => {
                // リレーは再接続して cursor から続きを読む（新しいプロセスにつながる）。
                let _ = socket.send(crate::handlers::streaming::restart_close_message()).await;
                break;
            }
            _ = ping_interval.tick() => {
                if socket.send(Message::Ping(vec![])).await.is_err() {
                    break;
                }
            }
            result = rx.recv() => {
                match result {
                    Ok(evt) => {
                        if socket.send(Message::Binary(evt.frame_bytes)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_n)) => {
                        tracing::warn!("[subscribeRepos] イベントチャンネルが遅延");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

#[cfg(test)]
mod repo_status_tests {
    use super::repo_status;

    #[test]
    fn maps_actor_state_to_repo_status() {
        assert_eq!(repo_status(false, false), (true, None));
        assert_eq!(repo_status(false, true), (false, Some("deactivated")));
        assert_eq!(repo_status(true, true), (false, Some("deleted")));
    }
}

use std::time::Duration;

use axum::{
    extract::{Query, State},
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Redirect},
};
use serde::Deserialize;
use sqlx::Row;
use tokio::sync::broadcast;

use seiran_common::atp::{
    build_commit_frame, build_identity_frame, cid_from_str, encode_car, CommitEvtOp,
};

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
        return ApiError::BadRequest("Unsupported hash function (expected sha2-256)".to_string()).into_response();
    }
    let sha256_hex = hex::encode(mh.digest());

    // media_files（ユーザーが投稿に添付した通常ファイル）に加え、atp_blobs
    // （com.atproto.repo.uploadBlob で受信・保存したバイト列。Bsky公式動画パイプラインが
    // トランスコード完了後に代理POSTしてくるものを含む）も検索する。
    let row = sqlx::query(
        "SELECT mime_type, url FROM (
             SELECT mf.mime_type AS mime_type,
                    rtrim(sp.public_url, '/') || '/' || mf.storage_key AS url
             FROM media_files mf
             JOIN storage_providers sp ON sp.id = mf.storage_provider_id
             WHERE mf.sha256 = $1
             UNION ALL
             SELECT ab.mime_type AS mime_type,
                    rtrim(sp.public_url, '/') || '/' || ab.storage_key AS url
             FROM atp_blobs ab
             JOIN storage_providers sp ON sp.id = ab.storage_provider_id
             WHERE ab.sha256 = $1
         ) t
         LIMIT 1",
    )
    .bind(&sha256_hex)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(r)) => {
            let url: String = r.try_get("url").unwrap_or_default();
            if url.is_empty() {
                return ApiError::NotFound("Blob not found").into_response();
            }
            Redirect::temporary(&url).into_response()
        }
        Ok(None) => ApiError::NotFound("Blob not found").into_response(),
        Err(e) => ApiError::Internal(format!("[getBlob] DB エラー: {}", e)).into_response(),
    }
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
            return ApiError::Internal(format!("[getRepo] アクター取得失敗: {}", e)).into_response();
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
            return ApiError::Internal(format!("[getRepo] ブロック取得失敗: {}", e)).into_response();
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
    let mut rx = state.atp_service.event_tx().subscribe();

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
                            tracing::error!("[subscribeRepos] frame_bytes 解凍失敗 id={}: {}", evt.id, e);
                        }
                    }
                }

                // frame_bytes が NULL の旧レコードは event_type に応じて再構築する
                let time_str = evt.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                let frame_result = if evt.event_type == "identity" {
                    let handle = evt.handle.as_deref().unwrap_or("");
                    build_identity_frame(evt.id, &evt.did, handle, &time_str)
                } else {
                    let commit_cid = match evt.commit_cid.as_deref().and_then(|s| cid_from_str(s).ok()) {
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
                    build_commit_frame(
                        evt.id, &evt.did, &commit_cid, prev_cid.as_ref(),
                        rev, evt.since_rev.as_deref(), car, &ops, &[], &time_str,
                        None,
                    )
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

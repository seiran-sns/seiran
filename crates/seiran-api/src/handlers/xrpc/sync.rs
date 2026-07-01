use axum::{
    extract::{Query, State},
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use tokio::sync::broadcast;

use seiran_common::atp::{
    build_commit_frame, cid_from_str, encode_car, CommitEvtOp,
};

use crate::AppState;

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
        Ok(None) => return (StatusCode::NOT_FOUND, "DID が見つかりません").into_response(),
        Err(e) => {
            eprintln!("[getRepo] アクター取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
        }
    };

    let actor_id = actor.id;
    let commit_cid_str = match actor.at_repo_cid {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, "リポジトリが未初期化です").into_response(),
    };

    let commit_cid = match cid_from_str(&commit_cid_str) {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "commit CID パース失敗").into_response(),
    };

    let block_rows = match state.atp_repo.find_blocks_by_actor(actor_id).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[getRepo] ブロック取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "ブロック取得失敗").into_response();
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
        Err(e) => {
            eprintln!("[getRepo] CAR 生成失敗: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "CAR 生成失敗").into_response()
        }
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

    if let Some(cursor) = params.cursor {
        let events = state
            .atp_repo
            .find_events_after(cursor, 500)
            .await
            .unwrap_or_default();

        for evt in events {
            let commit_cid = match cid_from_str(&evt.commit_cid) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let prev_cid = evt.prev_cid.as_deref().and_then(|s| cid_from_str(s).ok());

            let ops: Vec<CommitEvtOp> = evt
                .ops_json
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|op| {
                    let action = op["action"].as_str()?.to_string();
                    let path = op["path"].as_str()?.to_string();
                    let cid = cid_from_str(op["cid"].as_str()?).ok()?;
                    Some(CommitEvtOp { action, path, cid })
                })
                .collect();

            let time_str = evt.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            if let Ok(frame) = build_commit_frame(
                evt.id,
                &evt.did,
                &commit_cid,
                prev_cid.as_ref(),
                &evt.rev,
                evt.since_rev.as_deref(),
                &evt.car_bytes,
                &ops,
                &time_str,
            ) {
                if socket.send(Message::Binary(frame)).await.is_err() {
                    return;
                }
            }
        }
    }

    loop {
        match rx.recv().await {
            Ok(evt) => {
                if socket
                    .send(Message::Binary(evt.frame_bytes))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_n)) => {
                eprintln!("[subscribeRepos] イベントチャンネルが遅延");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

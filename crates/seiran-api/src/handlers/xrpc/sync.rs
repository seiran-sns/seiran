use axum::{
    extract::{Query, State},
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use sqlx::Row;
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
    let actor_row = sqlx::query(
        "SELECT id, at_repo_cid FROM actors WHERE at_did = $1 LIMIT 1",
    )
    .bind(&params.did)
    .fetch_optional(&state.db)
    .await;

    let (actor_id, commit_cid_str) = match actor_row {
        Ok(Some(r)) => {
            let id: i64 = match r.try_get("id") {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[getRepo] id 取得失敗: {}", e);
                    return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
                }
            };
            let cid: Option<String> = match r.try_get("at_repo_cid") {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[getRepo] at_repo_cid 取得失敗: {}", e);
                    return (StatusCode::INTERNAL_SERVER_ERROR, "DB エラー").into_response();
                }
            };
            (id, cid)
        }
        _ => return (StatusCode::NOT_FOUND, "DID が見つかりません").into_response(),
    };

    let commit_cid_str = match commit_cid_str {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, "リポジトリが未初期化です").into_response(),
    };

    let commit_cid = match cid_from_str(&commit_cid_str) {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "commit CID パース失敗").into_response(),
    };

    let block_rows = sqlx::query(
        "SELECT cid, bytes FROM atp_blocks WHERE actor_id = $1",
    )
    .bind(actor_id)
    .fetch_all(&state.db)
    .await;

    let block_rows = match block_rows {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[getRepo] ブロック取得失敗: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "ブロック取得失敗").into_response();
        }
    };

    let blocks: Vec<_> = block_rows
        .iter()
        .filter_map(|row| {
            let cid_str: String = row.try_get("cid").ok()?;
            let bytes: Vec<u8> = row.try_get("bytes").ok()?;
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
        let rows = sqlx::query(
            "SELECT id, car_bytes, did, commit_cid, prev_cid, rev, since_rev, ops_json, created_at
             FROM atp_repo_events WHERE id > $1 ORDER BY id ASC LIMIT 500",
        )
        .bind(cursor)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

        for row in rows {
            let seq: i64 = match row.try_get("id") {
                Ok(v) => v,
                Err(_) => continue,
            };
            let did: String = match row.try_get("did") {
                Ok(v) => v,
                Err(_) => continue,
            };
            let commit_cid_str: String = match row.try_get("commit_cid") {
                Ok(v) => v,
                Err(_) => continue,
            };
            let prev_cid_str: Option<String> = row.try_get("prev_cid").ok().flatten();
            let rev: String = match row.try_get("rev") {
                Ok(v) => v,
                Err(_) => continue,
            };
            let since_rev: Option<String> = row.try_get("since_rev").ok().flatten();
            let car_bytes: Vec<u8> = match row.try_get("car_bytes") {
                Ok(v) => v,
                Err(_) => continue,
            };
            let ops_json: serde_json::Value =
                row.try_get("ops_json").unwrap_or(serde_json::json!([]));
            let created_at: chrono::DateTime<chrono::Utc> =
                row.try_get("created_at").unwrap_or_else(|_| chrono::Utc::now());

            let commit_cid = match cid_from_str(&commit_cid_str) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let prev_cid = prev_cid_str.as_deref().and_then(|s| cid_from_str(s).ok());

            let ops: Vec<CommitEvtOp> = ops_json
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

            let time_str = created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            if let Ok(frame) = build_commit_frame(
                seq,
                &did,
                &commit_cid,
                prev_cid.as_ref(),
                &rev,
                since_rev.as_deref(),
                &car_bytes,
                &ops,
                &time_str,
            ) {
                if socket.send(Message::Binary(frame.into())).await.is_err() {
                    return;
                }
            }
        }
    }

    loop {
        match rx.recv().await {
            Ok(evt) => {
                if socket
                    .send(Message::Binary(evt.frame_bytes.into()))
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

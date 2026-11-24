//! outbox・featured 等の AP コレクション応答で共有する組み立て部品。
//! Note 自体は`seiran_common::ap::deliver::build_local_note`が組み立てる（配送と同じもの）。

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use crate::AppState;

pub const AS_PUBLIC: &str = "https://www.w3.org/ns/activitystreams#Public";

/// `application/activity+json` の 200 応答。
pub fn activity_json(body: serde_json::Value) -> Response {
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/activity+json",
        )],
        Json(body),
    )
        .into_response()
}

/// ローカル投稿の Note を、配送・`GET /notes/:id` と同じ`build_local_note`で組み立てる。
/// Note にならない行（Announce で配送したリポスト等）や組み立てに失敗した行は`None`。
pub async fn local_note(
    state: &AppState,
    post_id: i64,
    actor_id: i64,
) -> Option<seiran_common::ap::deliver::LocalNote> {
    match seiran_common::ap::deliver::build_local_note(
        &state.ap_client,
        &state.db,
        &state.local_domain,
        post_id,
        actor_id,
    )
    .await
    {
        Ok(note) => note,
        Err(e) => {
            tracing::warn!(
                "[ap_collection] Note 組み立て失敗 post_id={}: {}",
                post_id,
                e
            );
            None
        }
    }
}

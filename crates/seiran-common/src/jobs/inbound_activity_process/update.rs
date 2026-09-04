use super::note_input::normalize_ap_poll;
use super::*;

/// `Update`アクティビティのうち、アンケート（`object.type == "Question"`）と、
/// `seiranPost.counterpartPostId`を持つ`Note`（#237、下記`handle_update_seiranpost`参照）
/// のみを受理する。本文再編集全般のUpdate（それ以外の`object.type == "Note"`）は
/// 別件のため今回は非対応、黙って無視する。Update(Question)を受理できたNoteは
/// `posts.poll_update_received`をtrueにし、以後`Job::PollFetch`（生存監視フォールバック）の
/// 対象から外す（送信元がpush型実装と判明したため）。
pub(super) async fn handle_update(
    activity: serde_json::Value,
    inbox: &InboxContext,
) -> Result<(), String> {
    let object = &activity["object"];
    if object["type"].as_str() == Some("Note") {
        return handle_update_seiranpost(&activity, inbox).await;
    }
    if object["type"].as_str() != Some("Question") {
        return Ok(());
    }
    let Some(question_id) = object["id"].as_str() else {
        return Ok(());
    };
    let Some(poll) = normalize_ap_poll(object) else {
        return Ok(());
    };

    let Some((post_id, post_author_id)) = inbox
        .post_repo
        .find_id_and_actor_by_ap_object_id(question_id)
        .await
        .map_err(|e| format!("Update: Question検索失敗: {}", e))?
    else {
        return Ok(());
    };

    // なりすまし対策: Update の送信元（HTTP Signature 検証済みの actor）が投稿者本人か確認する
    // （`delete.rs::handle_delete`と同じパターン）。
    let actor_uri = activity["actor"].as_str().unwrap_or("");
    let sender = inbox
        .actor_repo
        .find_by_ap_uri(actor_uri)
        .await
        .map_err(|e| format!("送信元アクター検索エラー: {}", e))?;
    if sender.map(|a| a.id) != Some(post_author_id) {
        tracing::warn!(
            "[Update] {} の送信元アクター({})が投稿の所有者と一致しないため無視します",
            question_id,
            actor_uri
        );
        return Ok(());
    }

    sqlx::query(
        "UPDATE posts SET poll = $2, poll_update_received = true, poll_fetched_at = now()
         WHERE id = $1",
    )
    .bind(post_id)
    .bind(&poll)
    .execute(&inbox.db_pool)
    .await
    .map_err(|e| format!("Update: poll更新失敗: {}", e))?;

    broadcast_poll_update(
        &inbox.stream_hub,
        inbox.follow_repo.as_ref(),
        post_id,
        post_author_id,
        &poll,
    )
    .await;

    Ok(())
}

/// `Update(Note)`の狭いスコープの受理（#237）: `seiranPost.counterpartPostId`のみを反映し、
/// マージ再判定を行う。本文・CW等の変更が同じUpdateに含まれていても無視する
/// （`posts`の他フィールドには反映しない、本文再編集全般のUpdate(Note)受理は引き続き未対応）。
/// `docs/protocols.md` 5節「配送側の制約（非対称、後からUpdateで補完）」参照。
async fn handle_update_seiranpost(
    activity: &serde_json::Value,
    inbox: &InboxContext,
) -> Result<(), String> {
    let object = &activity["object"];
    let Some(ap_object_id) = object["id"].as_str() else {
        return Ok(());
    };
    let Some(seiran_post) = crate::seiran_post::SeiranPost::extract(object) else {
        return Ok(());
    };
    let Some(at_uri) = seiran_post.counterpart_post_id.as_deref() else {
        return Ok(());
    };

    let Some((post_id, post_author_id, current_at_uri)) = sqlx::query_as::<_, (i64, i64, Option<String>)>(
        "SELECT id, actor_id, at_uri FROM posts WHERE ap_object_id = $1",
    )
    .bind(ap_object_id)
    .fetch_optional(&inbox.db_pool)
    .await
    .map_err(|e| format!("Update(Note): posts検索失敗: {}", e))?
    else {
        // 対応するCreateがまだ無い（届いていない・処理中）。この場合は無視する
        // （`Delete`ハンドラと同様、対象が無ければ何もしない）。
        return Ok(());
    };

    // なりすまし対策: Update の送信元（HTTP Signature 検証済みの actor）が投稿者本人か確認する
    // （`handle_update`のQuestion分岐・`delete.rs::handle_delete`と同じパターン）。
    let actor_uri = activity["actor"].as_str().unwrap_or("");
    let sender = inbox
        .actor_repo
        .find_by_ap_uri(actor_uri)
        .await
        .map_err(|e| format!("送信元アクター検索エラー: {}", e))?;
    if sender.map(|a| a.id) != Some(post_author_id) {
        tracing::warn!(
            "[Update(Note)] {} の送信元アクター({})が投稿の所有者と一致しないため無視します",
            ap_object_id,
            actor_uri
        );
        return Ok(());
    }

    if current_at_uri.is_some() {
        // 既に確定済み（何らかの経路で既にマージ済み等）。冪等に無視する。
        return Ok(());
    }

    // #237 相互一致マージ判定。`insert_remote_with_dedup`/Jetstream `save_bsky_post`と同じ
    // advisory lock名前空間（key1=2、key2=hashtext(ap_object_id)）でDB反映を直列化する。
    let mut tx = inbox
        .db_pool
        .begin()
        .await
        .map_err(|e| format!("トランザクション開始失敗: {}", e))?;
    sqlx::query("SELECT pg_advisory_xact_lock(2, hashtext($1))")
        .bind(ap_object_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("advisory lock取得失敗: {}", e))?;

    // まず自分自身の申告を記録する（この場でマッチする相手が見つからなくても、
    // 将来ATP側が到着した際に見つけられるようにするため）。
    sqlx::query("UPDATE posts SET claimed_at_uri = $1 WHERE id = $2")
        .bind(at_uri)
        .bind(post_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("claimed_at_uri更新失敗: {}", e))?;

    let candidate: Option<(i64, i64, Option<String>)> = sqlx::query_as(
        "SELECT id, actor_id, claimed_ap_object_id FROM posts WHERE at_uri = $1",
    )
    .bind(at_uri)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| format!("マージ候補検索失敗: {}", e))?;

    let merge_target = candidate.and_then(|(doomed_id, doomed_actor_id, claimed_ap_object_id)| {
        let mutual_match = claimed_ap_object_id.as_deref() == Some(ap_object_id);
        // 投稿者一貫性チェック（簡略版、5節参照）: 両投稿の投稿者が既に同一actor行に
        // 解決されている場合のみマージする。オンメモリなアクター結婚は未実装のため、
        // 不一致ならマージせず孤立行のまま残す。
        (mutual_match && doomed_actor_id == post_author_id).then_some(doomed_id)
    });

    tx.commit()
        .await
        .map_err(|e| format!("トランザクションコミット失敗: {}", e))?;

    let Some(doomed_id) = merge_target else {
        return Ok(());
    };

    inbox
        .post_repo
        .finalize_post_merge(post_id, doomed_id, ap_object_id, at_uri)
        .await
        .map_err(|e| format!("finalize_post_merge失敗: {}", e))?;

    if let Err(e) = inbox
        .queue
        .enqueue(
            Job::PostMergeCleanup {
                survivor_post_id: post_id,
                doomed_post_id: doomed_id,
            },
            priority::LOW,
        )
        .await
    {
        tracing::error!("[Update(Note)] PostMergeCleanup enqueue失敗（続行）: {}", e);
    }

    tracing::info!(
        "[Update(Note)] seiranPostマージ成立: survivor={} doomed={}",
        post_id,
        doomed_id
    );
    Ok(())
}

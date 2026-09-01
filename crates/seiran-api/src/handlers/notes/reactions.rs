use super::*;
use validation::{validate_reaction_content, ReactionContent};

/// よく使う絵文字ピッカーで表示する候補数の上限。
const FREQUENT_REACTIONS_LIMIT: i64 = 24;

/// GET /api/reactions/frequent
/// 自分がよく使う絵文字（Unicode/カスタム問わず）を頻度順に返す（絵文字ピッカーの
/// 「よく使う」タブ用）。`reactions` が 1投稿1リアクションで切替時に上書きされる都合上、
/// これは「過去の使用履歴」ではなく「現在も自分が付けているリアクション」の集計になる
/// （`ReactionRepository::aggregate_for_actor` 参照）。
pub async fn frequent_reactions(
    me: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let rows = state
        .reactions
        .aggregate_for_actor(me.actor_id, FREQUENT_REACTIONS_LIMIT)
        .await
        .unwrap_or_default();
    let items: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(content, count, emoji_url)| {
            serde_json::json!({ "content": content, "count": count, "emojiUrl": emoji_url })
        })
        .collect();
    Json(serde_json::json!({ "items": items }))
}

/// リアクションチップのホバーポップオーバーに表示するアクター数の上限。
const REACTION_ACTORS_LIMIT: i64 = 50;

/// GET /api/notes/:id/reactions/:content/actors
/// 指定リアクション（絵文字/`:shortcode:`）を付けたアクター一覧を返す（ホバーポップオーバー用）。
/// 投稿の可視性チェックは `get_note` と同じ `find_by_id_for_viewer` を使う。
pub async fn reaction_actors(
    Path((note_id_str, content)): Path<(String, String)>,
    MaybeAuthedUser(user): MaybeAuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let my_actor_id = user.map(|u| u.actor_id);

    let note_id: i64 = match note_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    match state
        .posts
        .find_by_id_for_viewer(note_id, my_actor_id)
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("ポスト取得失敗: {}", e)).into_response(),
    };

    let actors = state
        .reactions
        .actors_for_reaction(note_id, &content, REACTION_ACTORS_LIMIT)
        .await
        .unwrap_or_default();

    Json(serde_json::json!({
        "actors": actors.into_iter().map(|a| serde_json::json!({
            "id": a.id.to_string(),
            "username": a.username,
            "domain": a.domain,
            "displayName": a.display_name,
            "avatarUrl": a.avatar_url,
        })).collect::<Vec<_>>(),
    }))
    .into_response()
}

/// GET /api/notes/:id/reposts
/// 対象ポストへのリポスト一覧を取得する（#226 リポストタブ）。取り消し済みも履歴として含む。
pub async fn note_reposts(
    Path(id): Path<String>,
    MaybeAuthedUser(user): MaybeAuthedUser,
    State(state): State<AppState>,
) -> Result<Json<dto::RepostListResponse>, ApiError> {
    let my_actor_id: Option<i64> = user.map(|u| u.actor_id);
    let post_id: i64 = id.parse().map_err(|_| ApiError::NotFound("NOT_FOUND"))?;

    state
        .posts
        .find_by_id_for_viewer(post_id, my_actor_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("NOT_FOUND"))?;

    let entries = state
        .posts
        .reposts_of(post_id, 100)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let reposts = entries
        .into_iter()
        .map(|e| dto::RepostEntryResponse {
            id: e.id.to_string(),
            user: dto::NoteUserInfo {
                id: e.actor_id.to_string(),
                username: e.username,
                domain: Some(e.domain),
                display_name: e.display_name,
                actor_type: e.actor_type,
                avatar_url: e.avatar_url,
                instance: None,
                follow_status: None,
                is_muted: None,
                is_blocking: None,
                is_blocked_by: None,
                is_repost_muted: None,
            },
            created_at: e.created_at.to_rfc3339(),
            deleted: e.deleted_at.is_some(),
        })
        .collect();

    Ok(Json(dto::RepostListResponse { reposts }))
}

/// POST /api/notes/:id/reactions
/// 自分の絵文字リアクションを追加する。ローカル保存に加え、AP（対象ポスト著者 + 自分の Fedi
/// フォロワー全員）・ATP（対象に at_uri がある場合）の双方へ配送する。
pub async fn create_reaction(
    Path(note_id_str): Path<String>,
    me: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<dto::ReactRequest>,
) -> impl IntoResponse {
    let note_id: i64 = match note_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    let parsed_content = match validate_reaction_content(&req.content) {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    // カスタム絵文字（`:shortcode:`）は custom_emojis に実在するか確認し、画像 URL を解決する。
    // Unicode 絵文字は emoji_url を持たない。
    let emoji_url = match &parsed_content {
        ReactionContent::Custom(shortcode) => {
            match state.emojis.find_url_by_shortcode(shortcode).await {
                Ok(Some(url)) => Some(url),
                Ok(None) => {
                    return ApiError::BadRequest("UNKNOWN_EMOJI".to_owned()).into_response()
                }
                Err(e) => {
                    return ApiError::Internal(format!("絵文字URL解決失敗: {}", e)).into_response()
                }
            }
        }
        ReactionContent::Unicode(_) => None,
    };
    let content = parsed_content.as_db_content();

    let post = match state
        .posts
        .find_by_id_for_viewer(note_id, Some(me.actor_id))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("ポスト取得失敗: {}", e)).into_response(),
    };

    if let Err(e) =
        crate::handlers::target_resolve::check_not_blocked(&state, me.actor_id, post.actor_id).await
    {
        return e.into_response();
    }

    // 切替時に取り消すべき旧リアクション（AP の Undo 対象 / ATP の削除対象 rkey）を退避。
    // 対象に ATP 実体が無ければ ATP 配信しない（AP/Bsky 由来でも at_uri を持たないポストへは無反応）。
    let prev = state
        .reactions
        .find_current(note_id, me.actor_id)
        .await
        .ok()
        .flatten();
    let delivery_meta = state.posts.find_delivery_meta(note_id).await.ok().flatten();

    // AP へ配送する Like/EmojiReact 自身の activity id を発行し、Undo で参照できるよう保存する。
    let activity_id = format!(
        "https://{}/activities/reactions/{}-{}-{}",
        state.local_domain,
        note_id,
        me.actor_id,
        chrono::Utc::now().timestamp_millis()
    );

    let new_reaction_id = generate_snowflake_id(chrono::Utc::now());
    let reaction_id = match state
        .reactions
        .insert(
            new_reaction_id,
            note_id,
            me.actor_id,
            "emoji",
            &content,
            Some(&activity_id),
            None,
            emoji_url.as_deref(),
        )
        .await
    {
        Ok(id) => id,
        Err(e) => {
            return ApiError::Internal(format!("reactions INSERT 失敗: {}", e)).into_response()
        }
    };

    // 通知ベル用（#37）: 自分の投稿への自作自演リアクションは通知しない。
    // `reaction_id` を渡しておくことで、対象ポストが ATP 実体を持つ場合に後段で ATP へ
    // コミットしたこのリアクションが自分自身の firehose 受信
    // （`seiran-atp-repo::firehose::handle_inbound_like_create`）で戻ってきても、
    // 同じ reaction_id を持つ通知が UNIQUE 制約で弾かれ、二重通知にならない
    // （`notifications.reaction_id`、`docs/protocols.md` 8節）。
    if post.actor_id != me.actor_id {
        state.stream_hub.publish_event(
            std::collections::HashSet::from([post.actor_id]),
            "reaction",
            serde_json::json!({
                "postId": note_id.to_string(),
                "emoji": content,
                "emojiUrl": emoji_url,
                "actor": { "username": me.username, "domain": me.domain, "displayName": me.display_name },
            }),
        );
        let notif_id = generate_snowflake_id(chrono::Utc::now());
        if let Err(e) = state
            .notifications
            .insert(
                notif_id,
                post.actor_id,
                NotificationKind::Reaction,
                Some(me.actor_id),
                Some(note_id),
                Some(&content),
                emoji_url.as_deref(),
                None,
                Some(reaction_id),
                None,
            )
            .await
        {
            tracing::error!("[create_reaction] notifications INSERT 失敗: {}", e);
        }
    }

    // タイムライン/ノート詳細のリアクション表示をリアルタイム更新する（Misskey 互換の
    // ストリーミング挙動に合わせる）。通知ベルと違い自作自演でも送出する。
    broadcast_reaction_update(
        &state.stream_hub,
        state.follows.as_ref(),
        state.reactions.as_ref(),
        note_id,
        post.actor_id,
        me.actor_id,
        Some(&content),
    )
    .await;

    // ATP 連携: 絵文字は送れないため Like として送る（`emoji` は非標準の拡張メタデータとして
    // ベストエフォートで載せる）。旧リアクションがあれば先に削除してから作り直す（切替）。
    if let Some(meta) = delivery_meta {
        if let (Some(target_uri), Some(target_cid)) = (meta.at_uri, meta.at_cid) {
            let atp = Arc::clone(&state.atp_service);
            let actor_id = me.actor_id;
            let emoji = content.clone();
            let prev_rkey = prev
                .as_ref()
                .and_then(|(_, _, at_uri, _)| at_uri.as_deref())
                .and_then(|u| u.rsplit('/').next())
                .map(|s| s.to_string());
            let now = chrono::Utc::now();
            tokio::spawn(async move {
                if let Some(rkey) = prev_rkey {
                    if let Err(e) = atp.delete_atp_like(actor_id, &rkey, now).await {
                        tracing::error!("[create_reaction] ATP Like 削除失敗（切替前処理）: {}", e);
                    }
                }
                if let Err(e) = atp
                    .commit_like(
                        actor_id,
                        note_id,
                        &target_uri,
                        &target_cid,
                        Some(&emoji),
                        reaction_id,
                        now,
                    )
                    .await
                {
                    tracing::error!("[create_reaction] ATP Like commit 失敗: {}", e);
                }
            });
        }
    }

    // AP 連携: 対象ポスト著者（Fedi リモートの場合のみ）+ 自分の Fedi フォロワー全員へ配送する。
    // 旧リアクションが既に AP へ配送済み（ap_activity_id あり）なら、ジョブ側が先に Undo してから送る（切替）。
    let undo_prev =
        prev.as_ref()
            .and_then(|(prev_content, prev_activity_id, _, prev_emoji_url)| {
                prev_activity_id.clone().map(|id| PrevApReaction {
                    activity_id: id,
                    content: prev_content.clone(),
                    emoji_url: prev_emoji_url.clone(),
                })
            });
    state
        .enqueue_ap_delivery(
            me.actor_id,
            ApDeliveryKind::Reaction {
                post_id: note_id,
                activity_id: activity_id.clone(),
                content: content.clone(),
                emoji_url: emoji_url.clone(),
                undo_prev,
            },
        )
        .await;

    let rmap = fetch_reactions_map(&state.db, &[note_id], Some(me.actor_id)).await;
    Json(serde_json::json!({
        "ok": true,
        "reactions": rmap.get(&note_id).cloned().unwrap_or_default(),
    }))
    .into_response()
}

/// DELETE /api/notes/:id/reactions/:content
/// 自分が付けたリアクションを取り消す。
pub async fn delete_reaction(
    Path((note_id_str, content)): Path<(String, String)>,
    user: AuthedUser,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let actor_id = user.actor_id;

    let note_id: i64 = match note_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_NOTE_ID".to_owned()).into_response(),
    };

    let post = match state
        .posts
        .find_by_id_for_viewer(note_id, Some(actor_id))
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return ApiError::NotFound("NOT_FOUND").into_response(),
        Err(e) => return ApiError::Internal(format!("ポスト取得失敗: {}", e)).into_response(),
    };

    // 削除前に現在の ap_activity_id（AP Undo 対象）と at_uri（ATP 削除対象の rkey）を退避しておく。
    let prev = state
        .reactions
        .find_current(note_id, actor_id)
        .await
        .ok()
        .flatten();

    let deleted = match state
        .reactions
        .delete_local(note_id, actor_id, &content)
        .await
    {
        Ok(n) => n,
        Err(e) => {
            return ApiError::Internal(format!("reactions DELETE 失敗: {}", e)).into_response()
        }
    };
    if deleted == 0 {
        return ApiError::NotFound("REACTION_NOT_FOUND").into_response();
    }

    broadcast_reaction_update(
        &state.stream_hub,
        state.follows.as_ref(),
        state.reactions.as_ref(),
        note_id,
        post.actor_id,
        actor_id,
        None,
    )
    .await;

    if let Some(rkey) = prev
        .as_ref()
        .and_then(|(_, _, at_uri, _)| at_uri.as_deref())
        .and_then(|u| u.rsplit('/').next())
        .map(|s| s.to_string())
    {
        let atp = Arc::clone(&state.atp_service);
        let now = chrono::Utc::now();
        tokio::spawn(async move {
            if let Err(e) = atp.delete_atp_like(actor_id, &rkey, now).await {
                tracing::error!("[delete_reaction] ATP Like 削除失敗: {}", e);
            }
        });
    }

    // AP 連携: 対象ポスト著者（Fedi リモートの場合のみ）+ 自分の Fedi フォロワー全員へ Undo を配送する。
    if let Some(prev_activity_id) = prev
        .as_ref()
        .and_then(|(_, ap_activity_id, _, _)| ap_activity_id.clone())
    {
        let emoji_url = prev
            .as_ref()
            .and_then(|(_, _, _, emoji_url)| emoji_url.clone());
        state
            .enqueue_ap_delivery(
                actor_id,
                ApDeliveryKind::UndoReaction {
                    post_id: note_id,
                    prev_activity_id,
                    content: content.clone(),
                    emoji_url,
                },
            )
            .await;
    }

    let rmap = fetch_reactions_map(&state.db, &[note_id], Some(actor_id)).await;
    Json(serde_json::json!({
        "ok": true,
        "reactions": rmap.get(&note_id).cloned().unwrap_or_default(),
    }))
    .into_response()
}

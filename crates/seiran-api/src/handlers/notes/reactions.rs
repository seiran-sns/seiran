use super::*;
use seiran_common::repository::{NewNotification, NewReaction, StoredReaction};
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
/// 閲覧者がミュート・ブロックしているアクターは一覧から除外する。
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
        .actors_for_reaction(note_id, &content, my_actor_id, REACTION_ACTORS_LIMIT)
        .await
        .unwrap_or_default();

    Json(serde_json::json!({
        "actors": actors.into_iter().map(|a| {
            let avatar_url = seiran_common::avatar::resolve_avatar_url(
                a.avatar_url,
                &a.actor_type,
                &a.domain,
                a.id,
            );
            serde_json::json!({
                "id": a.id.to_string(),
                "username": a.username,
                "domain": a.domain,
                "displayName": a.display_name,
                "avatarUrl": avatar_url,
            })
        }).collect::<Vec<_>>(),
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
        .map(|e| {
            let avatar_url = seiran_common::avatar::resolve_avatar_url(
                e.avatar_url,
                &e.actor_type,
                &e.domain,
                e.actor_id,
            );
            dto::RepostEntryResponse {
                id: e.id.to_string(),
                user: dto::NoteUserInfo {
                    id: e.actor_id.to_string(),
                    username: e.username,
                    domain: Some(e.domain),
                    display_name: e.display_name,
                    actor_type: e.actor_type,
                    avatar_url,
                    instance: None,
                    follow_status: None,
                    is_muted: None,
                    is_blocking: None,
                    is_blocked_by: None,
                    is_repost_muted: None,
                },
                created_at: e.created_at.to_rfc3339(),
                deleted: e.deleted_at.is_some(),
            }
        })
        .collect();

    Ok(Json(dto::RepostListResponse { reposts }))
}

/// リアクション内容を検証し、DB保存用の内容とカスタム絵文字の画像URLを返す。
/// カスタム絵文字（`:shortcode:`）は custom_emojis に実在するか確認する。
async fn resolve_reaction_content(
    state: &AppState,
    raw: &str,
) -> Result<(String, Option<String>), ApiError> {
    let parsed = validate_reaction_content(raw)?;
    let emoji_url = match &parsed {
        ReactionContent::Custom(shortcode) => Some(
            state
                .emojis
                .find_url_by_shortcode(shortcode)
                .await
                .map_err(|e| ApiError::Internal(format!("絵文字URL解決失敗: {}", e)))?
                .ok_or_else(|| ApiError::BadRequest("UNKNOWN_EMOJI".to_owned()))?,
        ),
        ReactionContent::Unicode(_) => None,
    };
    Ok((parsed.as_db_content(), emoji_url))
}

/// 閲覧可能なポストを取得する（リアクション作成・取消の共通前処理）。
async fn find_reactable_post(
    state: &AppState,
    note_id: i64,
    actor_id: i64,
) -> Result<TimelinePost, ApiError> {
    state
        .posts
        .find_by_id_for_viewer(note_id, Some(actor_id))
        .await
        .map_err(|e| ApiError::Internal(format!("ポスト取得失敗: {}", e)))?
        .ok_or(ApiError::NotFound("NOT_FOUND"))
}

/// タイムライン/ノート詳細のリアクション表示をリアルタイム更新する（Misskey 互換の
/// ストリーミング挙動に合わせる）。通知ベルと違い自作自演でも送出する。DM
/// （`direct`）はフォロワーへ漏れないよう配信先を参加者のみに絞った専用版を使う。
/// `content` は付けたリアクション（取り消しなら `None`）。
async fn broadcast_reaction_change(
    state: &AppState,
    post: &TimelinePost,
    reactor_actor_id: i64,
    content: Option<&str>,
) {
    if post.visibility == "direct" {
        broadcast_dm_reaction_update(
            &state.stream_hub,
            state.dm.as_ref(),
            state.reactions.as_ref(),
            post.id,
            post.actor_id,
            reactor_actor_id,
            content,
        )
        .await;
    } else {
        broadcast_reaction_update(
            &state.stream_hub,
            state.follows.as_ref(),
            state.reactions.as_ref(),
            post.id,
            post.actor_id,
            reactor_actor_id,
            content,
        )
        .await;
    }
}

/// 投稿者へのリアクション通知（ストリーミングの`reaction`イベント＋通知ベル、#37）。
/// 自分の投稿への自作自演リアクションは通知しない。`reaction_id` を渡しておくことで、
/// 対象ポストが ATP 実体を持つ場合に後段で ATP へコミットしたこのリアクションが自分自身の
/// firehose 受信（`seiran-atp-repo::firehose::handle_inbound_like_create`）で戻ってきても、
/// 同じ reaction_id を持つ通知が UNIQUE 制約で弾かれ、二重通知にならない
/// （`notifications.reaction_id`、`docs/protocols.md` 8節）。
async fn notify_reaction(
    state: &AppState,
    me: &AuthedUser,
    post: &TimelinePost,
    content: &str,
    emoji_url: Option<&str>,
    reaction_id: i64,
) {
    if post.actor_id == me.actor_id {
        return;
    }
    state.stream_hub.publish_event(
        std::collections::HashSet::from([post.actor_id]),
        "reaction",
        serde_json::json!({
            "postId": post.id.to_string(),
            "emoji": content,
            "emojiUrl": emoji_url,
            "actor": { "username": me.username, "domain": me.domain, "displayName": me.display_name },
        }),
    );
    let notification = NewNotification {
        notifier_actor_id: Some(me.actor_id),
        note_id: Some(post.id),
        reaction: Some(content),
        reaction_emoji_url: emoji_url,
        reaction_id: Some(reaction_id),
        ..NewNotification::new(
            generate_snowflake_id(chrono::Utc::now()),
            post.actor_id,
            NotificationKind::Reaction,
        )
    };
    if let Err(e) = state.notifications.insert(&notification).await {
        tracing::error!("[create_reaction] notifications INSERT 失敗: {}", e);
    }
}

/// ATP 連携: 絵文字は送れないため Like として送る（`emoji` は非標準の拡張メタデータとして
/// ベストエフォートで載せる）。旧リアクションがあれば先に削除してから作り直す（切替）。
/// 対象に ATP 実体が無ければ ATP 配信しない（AP/Bsky 由来でも at_uri を持たないポストへは無反応）。
fn deliver_reaction_to_atp(
    state: &AppState,
    actor_id: i64,
    post_id: i64,
    content: &str,
    reaction_id: i64,
    previous: Option<&StoredReaction>,
) {
    let atp = Arc::clone(&state.atp_service);
    let posts = Arc::clone(&state.posts);
    let emoji = content.to_owned();
    let prev_rkey = previous
        .and_then(StoredReaction::atp_rkey)
        .map(str::to_owned);
    let now = chrono::Utc::now();
    tokio::spawn(async move {
        let Some(meta) = posts.find_delivery_meta(post_id).await.ok().flatten() else {
            return;
        };
        let (Some(target_uri), Some(target_cid)) = (meta.at_uri, meta.at_cid) else {
            return;
        };
        if let Some(rkey) = prev_rkey {
            if let Err(e) = atp.delete_atp_like(actor_id, &rkey, now).await {
                tracing::error!("[create_reaction] ATP Like 削除失敗（切替前処理）: {}", e);
            }
        }
        if let Err(e) = atp
            .commit_like(
                actor_id,
                &seiran_common::atp::service::LikeCommit {
                    post_id,
                    target_at_uri: &target_uri,
                    target_at_cid: &target_cid,
                    emoji: Some(&emoji),
                    reaction_id,
                },
                now,
            )
            .await
        {
            tracing::error!("[create_reaction] ATP Like commit 失敗: {}", e);
        }
    });
}

/// 取り消したリアクションの ATP Like レコードを削除する。
fn delete_reaction_from_atp(state: &AppState, actor_id: i64, removed: &StoredReaction) {
    let Some(rkey) = removed.atp_rkey().map(str::to_owned) else {
        return;
    };
    let atp = Arc::clone(&state.atp_service);
    let now = chrono::Utc::now();
    tokio::spawn(async move {
        if let Err(e) = atp.delete_atp_like(actor_id, &rkey, now).await {
            tracing::error!("[delete_reaction] ATP Like 削除失敗: {}", e);
        }
    });
}

async fn reactions_response(state: &AppState, note_id: i64, actor_id: i64) -> Response {
    let rmap = fetch_reactions_map(&state.db, &[note_id], Some(actor_id)).await;
    Json(serde_json::json!({
        "ok": true,
        "reactions": rmap.get(&note_id).cloned().unwrap_or_default(),
    }))
    .into_response()
}

/// POST /api/notes/:id/reactions
/// 自分の絵文字リアクションを追加する（既に付けていれば切り替える）。ローカル保存に加え、
/// AP（対象ポスト著者 + 自分の Fedi フォロワー全員）・ATP（対象に at_uri がある場合）の双方へ配送する。
pub async fn create_reaction(
    Path(note_id_str): Path<String>,
    me: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<dto::ReactRequest>,
) -> Response {
    match create_reaction_inner(&note_id_str, &me, &state, &req.content).await {
        Ok(note_id) => reactions_response(&state, note_id, me.actor_id).await,
        Err(e) => e.into_response(),
    }
}

async fn create_reaction_inner(
    note_id_str: &str,
    me: &AuthedUser,
    state: &AppState,
    raw_content: &str,
) -> Result<i64, ApiError> {
    me.require_not_did_moved_out()?;
    let note_id: i64 = note_id_str
        .parse()
        .map_err(|_| ApiError::BadRequest("INVALID_NOTE_ID".to_owned()))?;
    let (content, emoji_url) = resolve_reaction_content(state, raw_content).await?;
    let post = find_reactable_post(state, note_id, me.actor_id).await?;
    crate::handlers::target_resolve::check_not_blocked(state, me.actor_id, post.actor_id).await?;

    // AP へ配送する Like/EmojiReact 自身の activity id を発行し、Undo で参照できるよう保存する。
    let activity_id = format!(
        "https://{}/activities/reactions/{}-{}-{}",
        state.local_domain,
        note_id,
        me.actor_id,
        chrono::Utc::now().timestamp_millis()
    );
    let saved = state
        .reactions
        .upsert(&NewReaction {
            id: generate_snowflake_id(chrono::Utc::now()),
            post_id: note_id,
            actor_id: me.actor_id,
            reaction_type: "emoji",
            content: &content,
            ap_activity_id: Some(&activity_id),
            at_uri: None,
            emoji_url: emoji_url.as_deref(),
        })
        .await
        .map_err(|e| ApiError::Internal(format!("reactions INSERT 失敗: {}", e)))?;

    notify_reaction(state, me, &post, &content, emoji_url.as_deref(), saved.id).await;
    broadcast_reaction_change(state, &post, me.actor_id, Some(&content)).await;
    deliver_reaction_to_atp(
        state,
        me.actor_id,
        note_id,
        &content,
        saved.id,
        saved.previous.as_ref(),
    );

    // AP 連携: 対象ポスト著者（Fedi リモートの場合のみ）+ 自分の Fedi フォロワー全員へ配送する。
    // 旧リアクションが既に AP へ配送済み（ap_activity_id あり）なら、ジョブ側が先に Undo してから送る（切替）。
    let undo_prev = saved.previous.and_then(|prev| {
        prev.ap_activity_id.map(|activity_id| PrevApReaction {
            activity_id,
            content: prev.content,
            emoji_url: prev.emoji_url,
        })
    });
    state
        .enqueue_ap_delivery(
            me.actor_id,
            ApDeliveryKind::Reaction {
                post_id: note_id,
                activity_id,
                content,
                emoji_url,
                undo_prev,
            },
        )
        .await;
    Ok(note_id)
}

/// DELETE /api/notes/:id/reactions/:content
/// 自分が付けたリアクションを取り消す。
pub async fn delete_reaction(
    Path((note_id_str, content)): Path<(String, String)>,
    user: AuthedUser,
    State(state): State<AppState>,
) -> Response {
    remove_reaction(&state, &user, &note_id_str, Some(&content)).await
}

/// リアクション取り消しの共通実装。`content`が`None`なら内容を問わず自分のリアクションを
/// 取り消す（Misskey互換API`notes/reactions/delete`用。`handlers::misskey`から呼ぶ）。
pub(crate) async fn remove_reaction(
    state: &AppState,
    user: &AuthedUser,
    note_id_str: &str,
    content: Option<&str>,
) -> Response {
    match remove_reaction_inner(state, user.actor_id, note_id_str, content).await {
        Ok(note_id) => reactions_response(state, note_id, user.actor_id).await,
        Err(e) => e.into_response(),
    }
}

async fn remove_reaction_inner(
    state: &AppState,
    actor_id: i64,
    note_id_str: &str,
    content: Option<&str>,
) -> Result<i64, ApiError> {
    let note_id: i64 = note_id_str
        .parse()
        .map_err(|_| ApiError::BadRequest("INVALID_NOTE_ID".to_owned()))?;
    let post = find_reactable_post(state, note_id, actor_id).await?;

    // 削除と、AP Undo / ATP 削除に必要な値の取得を1文で行う（別々に読むと切替と競合した際に
    // 取り消す対象がずれる）。
    let removed = state
        .reactions
        .delete_local(note_id, actor_id, content)
        .await
        .map_err(|e| ApiError::Internal(format!("reactions DELETE 失敗: {}", e)))?
        .ok_or(ApiError::NotFound("REACTION_NOT_FOUND"))?;

    broadcast_reaction_change(state, &post, actor_id, None).await;
    delete_reaction_from_atp(state, actor_id, &removed);

    // AP 連携: 対象ポスト著者（Fedi リモートの場合のみ）+ 自分の Fedi フォロワー全員へ Undo を配送する。
    if let Some(prev_activity_id) = removed.ap_activity_id {
        state
            .enqueue_ap_delivery(
                actor_id,
                ApDeliveryKind::UndoReaction {
                    post_id: note_id,
                    prev_activity_id,
                    content: removed.content,
                    emoji_url: removed.emoji_url,
                },
            )
            .await;
    }
    Ok(note_id)
}

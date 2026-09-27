use super::*;
use delivery::{
    broadcast_new_note, classify_post, deliver_regular_post, deliver_repost,
    redirect_bridge_post_meta, resolve_quote_embed, resolve_reply_context,
    with_bridge_delivery_targets, DeliveryTargets, RegularPostDelivery, ReplyContext,
};
use seiran_common::repository::NewNotification;
use validation::{
    validate_attachment_ids, validate_cw, validate_dm_text_length, validate_link_card_urls,
    validate_poll_choices, validate_text_length,
};

/// 検証済みの添付ファイル ID 群を投稿に紐付ける。
pub(crate) async fn attach_media_files(
    state: &AppState,
    post_id: i64,
    attachment_ids: &[i64],
) -> Result<(), ApiError> {
    for (position, media_file_id) in attachment_ids.iter().enumerate() {
        state
            .posts
            .attach_media(post_id, *media_file_id, position as i16)
            .await
            .map_err(|e| ApiError::Internal(format!("添付 INSERT 失敗: {}", e)))?;
    }
    Ok(())
}

/// リポスト作成（`renote_id` 指定時）を処理する。
/// 元ポストのメタ情報取得 → repost レコード挿入 → 両プロトコルへの配送 → realtime 配信、の順で行う。
async fn create_repost(
    state: &AppState,
    actor_id: i64,
    username: String,
    display_name: Option<String>,
    renote_id_str: &str,
    req: &CreateNoteRequest,
    now: chrono::DateTime<chrono::Utc>,
) -> Response {
    let renote_id: i64 = match renote_id_str.parse() {
        Ok(id) => id,
        Err(_) => return ApiError::BadRequest("INVALID_RENOTE_ID".to_owned()).into_response(),
    };

    let meta = match state.posts.find_delivery_meta(renote_id).await {
        Ok(Some(m)) => m,
        Ok(None) => return ApiError::NotFound("RENOTE_TARGET_NOT_FOUND").into_response(),
        Err(e) => {
            return ApiError::Internal(format!("repost 元ポスト取得失敗: {}", e)).into_response()
        }
    };
    // ブリッジポスト対応: リポスト対象がブリッジポストなら、内部的には常に元ポストへの
    // リポストとして扱う（`crate::bridge_post`・`docs/protocols.md`参照）。
    let (renote_id, meta) = redirect_bridge_post_meta(state, renote_id, meta).await;

    // Misskey/Mastodon 互換: 非公開（followers_only）ポストはリポスト禁止。
    // `direct` も同様に厳格扱いする（閲覧制御が両者を同列に扱っているのに合わせる）。
    // 新規操作の明示的な拒否のため、投稿時のような「黙った読み替え」ではなく通常のエラーを返す。
    if meta.visibility == "followers_only" || meta.visibility == "direct" {
        return ApiError::Forbidden("PRIVATE_POST_NOT_REPOSTABLE").into_response();
    }

    // 元ポストの投稿者とブロック関係にある場合はリポストを拒否する（Bsky準拠ブロック、双方向）。
    if let Err(e) =
        crate::handlers::target_resolve::check_not_blocked(state, actor_id, meta.actor_id).await
    {
        return e.into_response();
    }

    // リポスト自身の可視性はクライアントが選べず、元ポストから自動決定する。
    // ここに到達する時点で meta.visibility は "public" か "unlisted" のいずれかのみ。
    let repost_visibility: &str = if meta.visibility == "unlisted" {
        "unlisted"
    } else {
        "public"
    };

    let origin = classify_post(
        meta.ap_object_id.as_deref(),
        meta.at_uri.as_deref(),
        meta.actor_type == "local",
    );
    // ブリッジポスト対応: 元ポストが対向プロトコル側に独立したブリッジポストを持つ場合、
    // 配送先識別子をそちらへ差し替える（`LocalOrSeiran`なら差し替えない）。
    let meta = with_bridge_delivery_targets(state, meta, origin).await;

    let post_id = generate_snowflake_id(now);
    // リポスト行の ap_object_id は、deliver_repost が実際に配送するActivity種別に合わせて
    // 決定する。元ポストに ap_object_id がある（Fedi 側にも実体がある）場合のみ実際に
    // Announce が配送されるため /announces/ 形式にし、元ポストが Bsky ネイティブ（at_uri
    // のみ）の場合は PostToFollowers フォールバックとして通常の Create(Note) が配送される
    // ため /notes/ 形式にする。ここが常に /announces/ 形式だと、DBが自称する身元と実際に
    // 配送された身元が食い違い、外部からの参照（ブースト等）で同一投稿と認識できず重複行が
    // 生成される。
    let ap_object_id = if meta.ap_object_id.is_some() {
        format!("https://{}/announces/{}", state.local_domain, post_id)
    } else {
        format!("https://{}/notes/{}", state.local_domain, post_id)
    };

    match state
        .posts
        .insert_repost(InsertRepostParams {
            id: post_id,
            actor_id,
            ap_object_id: &ap_object_id,
            repost_of_post_id: Some(renote_id),
            repost_of_ap_uri: None,
            repost_of_ref_status: None,
            created_at: now,
            visibility: repost_visibility,
        })
        .await
    {
        Ok(()) => {}
        Err(sqlx::Error::Database(ref db_err)) if db_err.code().as_deref() == Some("23505") => {
            // UNIQUE 制約違反 = すでにリポスト済み
            return ApiError::Conflict("ALREADY_REPOSTED").into_response();
        }
        Err(e) => {
            return ApiError::Internal(format!("repost INSERT 失敗: {}", e)).into_response();
        }
    }

    deliver_repost(
        state,
        post_id,
        actor_id,
        now,
        DeliveryTargets {
            fedi: req.deliver_to_fedi.unwrap_or(true),
            bsky: req.deliver_to_bsky.unwrap_or(true),
        },
        &meta,
        origin,
    )
    .await;

    // リポスト通知: ローカルユーザーの投稿が他ユーザーにリポストされた場合に作る。
    if meta.actor_type == "local" && meta.actor_id != actor_id {
        state.stream_hub.publish_event(
            std::collections::HashSet::from([meta.actor_id]),
            "repost",
            serde_json::json!({
                "postId": post_id.to_string(),
                "actor": { "username": username, "domain": serde_json::Value::Null }
            }),
        );
        let notif_id = generate_snowflake_id(chrono::Utc::now());
        if let Err(e) = state
            .notifications
            .insert(&NewNotification {
                notifier_actor_id: Some(actor_id),
                note_id: Some(post_id),
                ..NewNotification::new(notif_id, meta.actor_id, NotificationKind::Repost)
            })
            .await
        {
            tracing::error!("[create_repost] notifications INSERT 失敗: {}", e);
        }
    }

    let avatar_url = state
        .actors
        .find_avatar_url(actor_id)
        .await
        .ok()
        .flatten()
        .or_else(|| {
            Some(seiran_common::avatar::fallback_avatar_url(
                &state.local_domain,
                actor_id,
            ))
        });
    let mut repost_resp = NoteResponse {
        id: post_id.to_string(),
        text: String::new(),
        created_at: now.to_rfc3339(),
        user: NoteUserInfo {
            id: actor_id.to_string(),
            username,
            domain: Some(state.local_domain.to_string()),
            display_name,
            actor_type: "local".to_string(),
            avatar_url,
            instance: None,
            follow_status: None,
            is_muted: None,
            is_blocking: None,
            is_blocked_by: None,
            is_repost_muted: None,
        },
        attachments: vec![],
        renote_id: Some(renote_id.to_string()),
        quote_id: None,
        reply_id: None,
        renote_status: None,
        quote_status: None,
        reply_status: None,
        parent_original_id: None,
        reactions: vec![],
        renote: None,
        quote: None,
        author_suspended: None, // 投稿作成主体は凍結中に投稿できないため常に非凍結
        reposted_by_me: None,
        emojis: HashMap::new(),
        pinned_by_me: None,
        // リポストラッパー自体は NoteCard 上で直接描画されない（renote 側の中身が表示される）
        // ため、配送先・可視性は未設定のままでよい。
        visibility: None,
        thread_root_post_id: None,
        recipients: None,
        deliver_fedi: None,
        deliver_bsky: None,
        reply_fedi_allowed: false,
        reply_bsky_allowed: false,
        reply_blocked: false,
        quote_blocked: false,
        remote_url: None,
        bridge_original_post_id: None,
        content_warning: None,
        poll: None,
        reply_count: 0,
        quote_count: 0,
        repost_count: 0,
        link_cards: vec![],
        content_html: None,
    };
    // 元ポストを埋め込んでから返す（#45: リポストカードの中身）。
    embed_renotes(
        &state.db,
        std::slice::from_mut(&mut repost_resp),
        Some(actor_id),
    )
    .await;
    embed_quotes(
        &state.db,
        std::slice::from_mut(&mut repost_resp),
        Some(actor_id),
    )
    .await;
    attach_reply_quote_gates(
        state,
        std::slice::from_mut(&mut repost_resp),
        Some(actor_id),
    )
    .await;
    attach_remote_instance_info(state, std::slice::from_mut(&mut repost_resp)).await;
    enqueue_stale_poll_fetches(state, std::slice::from_ref(&repost_resp)).await;
    broadcast_new_note(state, actor_id, &repost_resp).await;

    Json(repost_resp).into_response()
}

/// 引用元の公開範囲（`quoted_vis`）と、新たに作成しようとする投稿の公開範囲（`new_vis`）から、
/// 引用の可否を検証する（#143）。
/// - プライベート投稿（`followers_only` / `direct`）は引用不可。
/// - ひかえめ投稿（`unlisted`）は引用投稿が `unlisted`, `followers_only`, `direct` の場合のみ許可。
/// - パブリック投稿（`public`）はすべての公開範囲から引用許可。
fn validate_quote_visibility(quoted_vis: &str, new_vis: &str) -> Result<(), ApiError> {
    match quoted_vis {
        "followers_only" | "direct" => Err(ApiError::Forbidden("PRIVATE_POST_NOT_QUOTABLE")),
        "unlisted" => {
            if new_vis == "public" {
                Err(ApiError::BadRequest(
                    "CANNOT_QUOTE_UNLISTED_PUBLICLY".to_owned(),
                ))
            } else {
                Ok(())
            }
        }
        _ => Ok(()),
    }
}

/// [`validate_create_regular_post_input`] が確定した値。ここから先の永続化・配送
/// （[`persist_regular_post`]）は、この構造体が既に妥当であることを前提にする
/// （＝もう一度バリデーションし直さない）。
struct ValidatedCreatePost<'a> {
    text: String,
    visibility: &'static str,
    deliver_fedi: bool,
    deliver_bsky: bool,
    recipient_actor_ids: Vec<i64>,
    reply_to_id_i64: Option<i64>,
    quote_of_id_i64: Option<i64>,
    quote_notif_recipient: Option<i64>,
    poll_json: Option<serde_json::Value>,
    content_warning: Option<String>,
    attachment_ids_i64: Vec<i64>,
    reply_ctx: ReplyContext,
    req: &'a CreateNoteRequest,
}

/// 通常投稿・リプライ・引用投稿の入力を検証し、可視性・配送先プロトコル・DM宛先・
/// 引用可否・スレッド起点IDを決定する（what）。DBへの書き込みは一切行わない
/// （引用元・DM宛先・重複メンションの参照読み取りのみ）。
async fn validate_create_regular_post_input<'a>(
    state: &AppState,
    actor_id: i64,
    req: &'a CreateNoteRequest,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<ValidatedCreatePost<'a>, ApiError> {
    let text = req.text.as_deref().unwrap_or("").to_string();
    if text.trim().is_empty() {
        return Err(ApiError::BadRequest("text は空にできません".to_owned()));
    }
    crate::rate_limit::check_post_rate_limit(state, actor_id).await?;

    let reply_ctx = match &req.reply_to_id {
        Some(id) => resolve_reply_context(state, id, actor_id).await?,
        None => ReplyContext::top_level(),
    };
    // 可視性の決定(リプライ先の制約を含む)。新規パラメータのため後方互換は考慮不要
    // (不正値はエラーでよい)。
    let visibility: &'static str = reply_ctx.resolve_visibility(req.visibility.as_deref())?;
    let recipient_actors = resolve_dm_recipients(state, req, visibility).await?;
    let recipient_actor_ids: Vec<i64> = recipient_actors.iter().map(|a| a.id).collect();
    let has_bsky_recipient = recipient_actors.iter().any(|a| a.actor_type == "bsky");
    let (deliver_fedi, deliver_bsky) =
        decide_delivery_targets(req, visibility, &reply_ctx, &recipient_actors, actor_id);

    validate_post_text_length(state, &text, visibility, has_bsky_recipient, deliver_bsky).await?;
    validate_attachments_and_embed_choice(req, visibility, has_bsky_recipient)?;
    let poll_json = build_poll_json(req, visibility, now)?;
    let content_warning = validate_content_warning(req, visibility)?;
    // ポスト言語（Bsky配送の`langs`にのみ意味を持つ、AP配送では使わない）。表示言語設定と
    // 同じ許可リストで検証する。Misskey互換APIクライアント等、本フィールドを送らない
    // クライアントとの後方互換のため`None`は許可し（従来通り言語情報なしで配送）、
    // `Some`だが未対応言語の場合のみ拒否する。
    if let Some(lang) = &req.language {
        if !seiran_common::is_supported_language(lang) {
            return Err(ApiError::BadRequest("UNSUPPORTED_LANGUAGE".to_owned()));
        }
    }
    // URLリンクカードのチェックボックス選択（Bsky embed選択のラジオボタンリストを出せない
    // 場合の代替、Bsky配送オフ or CW中）。
    validate_link_card_urls(&req.link_card_urls)?;

    let reply_to_id_i64: Option<i64> = req.reply_to_id.as_deref().and_then(|s| s.parse().ok());
    let (quote_of_id_i64, quote_notif_recipient) =
        match req.quote_of_id.as_deref().and_then(|s| s.parse().ok()) {
            Some(quote_id) => {
                let (id, recipient) =
                    resolve_quote_target(state, actor_id, quote_id, visibility).await?;
                (Some(id), recipient)
            }
            None => (None, None),
        };

    if visibility != "direct" {
        let mut contact_targets =
            extract_local_mention_actor_ids(&text, &state.local_domain, &state.db).await;
        contact_targets.extend(reply_ctx.parent_local_actor_id);
        contact_targets.extend(quote_notif_recipient);
        crate::rate_limit::check_and_record_contacts(state, actor_id, contact_targets).await?;
    }
    // DM(direct)宛先とブロック関係にある場合は送信を拒否する。
    for recipient in &recipient_actors {
        crate::handlers::target_resolve::check_not_blocked(state, actor_id, recipient.id).await?;
    }

    // attachment_ids を i64 に変換（バリデーション済みなので unwrap 安全）
    let attachment_ids_i64: Vec<i64> = req
        .attachment_ids
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|s| s.parse::<i64>().unwrap())
        .collect();

    Ok(ValidatedCreatePost {
        text,
        visibility,
        deliver_fedi,
        deliver_bsky,
        recipient_actor_ids,
        reply_to_id_i64,
        quote_of_id_i64,
        quote_notif_recipient,
        poll_json,
        content_warning,
        attachment_ids_i64,
        reply_ctx,
        req,
    })
}

/// DM(visibility=="direct")の宛先を解決・検証する（direct以外は空）。宛先IDは指定順を保つ。
/// Bsky の DM は1対1のみのため、Bsky宛先が1人でも含まれるなら他の宛先の同居を許さない。
async fn resolve_dm_recipients(
    state: &AppState,
    req: &CreateNoteRequest,
    visibility: &str,
) -> Result<Vec<Actor>, ApiError> {
    if visibility != "direct" {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = match req.recipient_actor_ids.as_deref() {
        Some(ids) if !ids.is_empty() => ids
            .iter()
            .map(|s| s.parse::<i64>())
            .collect::<Result<_, _>>()
            .map_err(|_| ApiError::BadRequest("INVALID_RECIPIENT_ACTOR_ID".to_owned()))?,
        _ => {
            return Err(ApiError::BadRequest(
                "RECIPIENT_ACTOR_IDS_REQUIRED".to_owned(),
            ))
        }
    };
    let found = state
        .actors
        .find_by_ids(&ids)
        .await
        .map_err(|e| ApiError::Internal(format!("DM宛先アクター取得失敗: {}", e)))?;
    // 指定順を保ったまま重複を除く。存在しないIDが含まれていれば拒否する（そのまま
    // `post_recipients`へ渡すと外部キー違反で500になる）。
    let mut by_id: HashMap<i64, Actor> = found.into_iter().map(|a| (a.id, a)).collect();
    let mut recipients: Vec<Actor> = Vec::with_capacity(ids.len());
    for id in &ids {
        if let Some(actor) = by_id.remove(id) {
            recipients.push(actor);
        } else if !recipients.iter().any(|a| a.id == *id) {
            return Err(ApiError::BadRequest(
                "INVALID_RECIPIENT_ACTOR_ID".to_owned(),
            ));
        }
    }
    let bsky_count = recipients.iter().filter(|a| a.actor_type == "bsky").count();
    if bsky_count >= 1 && recipients.len() > 1 {
        return Err(ApiError::BadRequest(
            "BSKY_DM_SINGLE_RECIPIENT_ONLY".to_owned(),
        ));
    }
    Ok(recipients)
}

/// 配送先プロトコル（Fedi, Bsky）を決める。
fn decide_delivery_targets(
    req: &CreateNoteRequest,
    visibility: &str,
    reply_ctx: &ReplyContext,
    recipient_actors: &[Actor],
    actor_id: i64,
) -> (bool, bool) {
    if visibility == "direct" {
        // remote_seiran（#236で相互申告マージが成立した他seiranサーバーのアクター）はAP経由でも
        // 受信できるため、fediと同様にAP DM配送のトリガーに含める（漏らすとDM自体が
        // どちらの配送ジョブもenqueueされず届かなくなる）。
        let has_fedi_recipient = recipient_actors
            .iter()
            .any(|a| a.actor_type == "fedi" || a.actor_type == "remote_seiran");
        let has_bsky_recipient = recipient_actors.iter().any(|a| a.actor_type == "bsky");
        return (has_fedi_recipient, has_bsky_recipient);
    }
    let deliver_fedi = req.deliver_to_fedi.unwrap_or(true) && reply_ctx.deliver_fedi_allowed;
    let mut deliver_bsky = req.deliver_to_bsky.unwrap_or(true) && reply_ctx.deliver_bsky_allowed;
    // Misskey互換API保護: Bsky はプロトコル上 followers_only（フォロワー限定）投稿を配信できない。
    // visibility が followers_only なのに Bsky 配送が要求された場合、エラーを返さず Fedi のみ
    // 配送に読み替える（unlisted は Bsky 配送可能。フロントは PostComposer で事前にブロックするが、
    // フロントを経由しない外部クライアントからの想定外リクエストにも安全に対応する）。
    if visibility == "followers_only" && deliver_bsky {
        tracing::info!(
            "[create_regular_post] visibility={} で Bsky 配送が要求されたため Fedi のみに読み替え（actor_id={}）",
            visibility, actor_id
        );
        deliver_bsky = false;
    }
    (deliver_fedi, deliver_bsky)
}

/// 本文の長さを検証する。DMの文字数上限はBsky宛先の有無で切り替える（通常投稿の上限とは
/// 別体系）。通常投稿で Bsky 配信する場合、メンション変換（`@user` → `@user.example.com` 等）で
/// バイト数・書記素数が増えうるため、変換後テキストを同期的に確定し、それに対して Bsky の厳密な
/// 上限（300 書記素・3000 バイト）を検証する（ここで弾けば DB への INSERT 自体が行われない）。
async fn validate_post_text_length(
    state: &AppState,
    text: &str,
    visibility: &str,
    has_bsky_recipient: bool,
    deliver_bsky: bool,
) -> Result<(), ApiError> {
    if visibility == "direct" {
        return validate_dm_text_length(text, has_bsky_recipient);
    }
    let bsky_text_for_validation: Option<String> = if deliver_bsky {
        let (bsky_text, _facets) = convert_mentions_for_bsky(
            text,
            &state.local_domain,
            &state.db,
            state.ap_client.http.as_ref(),
        )
        .await;
        Some(bsky_text)
    } else {
        None
    };
    validate_text_length(text, bsky_text_for_validation.as_deref())
}

/// 添付IDと Bsky embed 選択を検証する。
fn validate_attachments_and_embed_choice(
    req: &CreateNoteRequest,
    visibility: &str,
    has_bsky_recipient: bool,
) -> Result<(), ApiError> {
    if let Some(ids) = &req.attachment_ids {
        validate_attachment_ids(ids)?;
        if visibility == "direct" && has_bsky_recipient && !ids.is_empty() {
            return Err(ApiError::BadRequest("BSKY_DM_NO_ATTACHMENTS".to_owned()));
        }
    }
    // Bsky embed選択（#227）: `Attachment{id}`を選んだ場合、そのidは今回の投稿の添付として
    // 実際に指定されていなければならない（含まれないidの選択は不正リクエストとして拒否する）。
    if let Some(dto::BskyEmbedChoice::Attachment { id }) = &req.bsky_embed_choice {
        let attached = req
            .attachment_ids
            .as_ref()
            .is_some_and(|ids| ids.iter().any(|i| i == id));
        if !attached {
            return Err(ApiError::BadRequest("INVALID_BSKY_EMBED_CHOICE".to_owned()));
        }
    }
    // Bsky embed選択（#228）: `Poll`を選んだ場合、このリクエストが実際にアンケートを
    // 作成していなければならない。
    if matches!(req.bsky_embed_choice, Some(dto::BskyEmbedChoice::Poll)) && req.poll.is_none() {
        return Err(ApiError::BadRequest("INVALID_BSKY_EMBED_CHOICE".to_owned()));
    }
    Ok(())
}

/// アンケート（#228）を`posts.poll`の形のJSONへ組み立てる。DMには馴染まないため禁止する
/// （BSKY_DM_NO_ATTACHMENTSと同じ理由）。期限は 絶対時刻（ISO8601） > Misskey互換epochミリ秒 >
/// 相対秒数 の優先順で解決し、いずれも無ければ無期限（endTimeを省略）。
fn build_poll_json(
    req: &CreateNoteRequest,
    visibility: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<serde_json::Value>, ApiError> {
    let Some(p) = &req.poll else {
        return Ok(None);
    };
    if visibility == "direct" {
        return Err(ApiError::BadRequest("POLL_NOT_ALLOWED_FOR_DM".to_owned()));
    }
    let choices = validate_poll_choices(&p.choices)?;
    let end_time: Option<chrono::DateTime<chrono::Utc>> = p
        .expires_at
        .as_deref()
        .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
        .or_else(|| {
            p.expires_at_epoch_ms
                .and_then(chrono::DateTime::from_timestamp_millis)
        })
        .or_else(|| {
            p.expires_in_seconds
                .map(|secs| now + chrono::Duration::seconds(secs))
        });
    Ok(Some(serde_json::json!({
        "multiple": p.multiple.unwrap_or(false),
        "options": choices.into_iter().map(|name| serde_json::json!({"name": name, "votes": 0})).collect::<Vec<_>>(),
        "endTime": end_time.map(|t| t.to_rfc3339()),
    })))
}

/// CW（閲覧注意、#229）を検証する。DMには馴染まないため禁止する（POLL_NOT_ALLOWED_FOR_DMと同じ理由）。
fn validate_content_warning(
    req: &CreateNoteRequest,
    visibility: &str,
) -> Result<Option<String>, ApiError> {
    let Some(cw) = &req.content_warning else {
        return Ok(None);
    };
    if visibility == "direct" {
        return Err(ApiError::BadRequest("CW_NOT_ALLOWED_FOR_DM".to_owned()));
    }
    validate_cw(cw).map(Some)
}

/// 引用先を検証し、（内部的な引用先ID, 引用通知の宛先）を返す。引用先とブロック関係にある場合、
/// および公開範囲制約違反の場合は拒否する。引用先がブリッジポストなら、内部的には常に元ポストへの
/// 引用として扱う（`crate::bridge_post`・`docs/protocols.md`参照）。
async fn resolve_quote_target(
    state: &AppState,
    actor_id: i64,
    quote_id: i64,
    visibility: &str,
) -> Result<(i64, Option<i64>), ApiError> {
    let meta = state
        .posts
        .find_delivery_meta(quote_id)
        .await
        .map_err(|e| ApiError::Internal(format!("引用元ポスト取得失敗: {}", e)))?
        .ok_or(ApiError::NotFound("QUOTE_TARGET_NOT_FOUND"))?;
    let (redirected_id, meta) = redirect_bridge_post_meta(state, quote_id, meta).await;
    crate::handlers::target_resolve::check_not_blocked(state, actor_id, meta.actor_id).await?;
    validate_quote_visibility(&meta.visibility, visibility)?;
    let notif_recipient =
        (meta.actor_type == "local" && meta.actor_id != actor_id).then_some(meta.actor_id);
    Ok((redirected_id, notif_recipient))
}

/// 通常投稿・リプライ・引用投稿を処理する（`renote_id` を持たないケース）。
/// 検証（[`validate_create_regular_post_input`]）→ 永続化・配送（[`persist_regular_post`]）の
/// 2段に委ねるだけの薄いオーケストレーション。
async fn create_regular_post(
    state: &AppState,
    actor_id: i64,
    username: String,
    display_name: Option<String>,
    req: &CreateNoteRequest,
    now: chrono::DateTime<chrono::Utc>,
) -> Response {
    let validated = match validate_create_regular_post_input(state, actor_id, req, now).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    persist_regular_post(state, actor_id, username, display_name, now, validated).await
}

/// [`validate_create_regular_post_input`] が確定した入力をもとに、投稿の永続化・
/// 通知生成・両プロトコルへの配送・realtime配信を行う（how）。もう一度バリデーションはしない。
/// ローカル投稿に起因する通知（リプライ・引用・メンション）を、リアルタイムイベント配信と
/// 通知レコード挿入の対で作る。3種で完全に同形のため共通化する（how）。
async fn notify_local_actor(
    state: &AppState,
    target_actor_id: i64,
    kind: NotificationKind,
    event_name: &str,
    actor_id: i64,
    post_id: i64,
    username: &str,
) {
    state.stream_hub.publish_event(
        std::collections::HashSet::from([target_actor_id]),
        event_name,
        serde_json::json!({
            "postId": post_id.to_string(),
            "actor": { "username": username, "domain": serde_json::Value::Null },
        }),
    );
    let notif_id = generate_snowflake_id(chrono::Utc::now());
    if let Err(e) = state
        .notifications
        .insert(&NewNotification {
            notifier_actor_id: Some(actor_id),
            note_id: Some(post_id),
            ..NewNotification::new(notif_id, target_actor_id, kind)
        })
        .await
    {
        tracing::error!(
            "[create_regular_post] {} notifications INSERT 失敗: {}",
            event_name,
            e
        );
    }
}

/// 本文中のカスタム絵文字（`:shortcode:`）を、このサーバーの `custom_emojis` と照合して
/// emoji_map を構築する。Fedi受信投稿はAPのtag配列由来でemoji_mapが埋まるが、ローカル
/// 投稿作成にはその経路が無いため、ここで解決しないと本文中のショートコードが常に画像化
/// されない（#77）。解決に失敗しても投稿自体は継続する（絵文字がテキストのまま出るだけ）。
async fn resolve_local_emoji_map(
    state: &AppState,
    text: &str,
) -> (serde_json::Value, HashMap<String, String>) {
    let shortcode_candidates = extract_shortcode_candidates(text);
    let local_emoji_pairs = if shortcode_candidates.is_empty() {
        Vec::new()
    } else {
        match state
            .emojis
            .find_urls_by_shortcodes(&shortcode_candidates)
            .await
        {
            Ok(pairs) => pairs,
            Err(e) => {
                tracing::error!("[create_regular_post] 絵文字ショートコード解決失敗: {}", e);
                Vec::new()
            }
        }
    };
    let local_emoji_map: serde_json::Value = serde_json::Value::Object(
        local_emoji_pairs
            .into_iter()
            .map(|(code, url)| (format!(":{}:", code), serde_json::Value::String(url)))
            .collect(),
    );
    let response_emojis: HashMap<String, String> = local_emoji_map
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    (local_emoji_map, response_emojis)
}

async fn persist_regular_post(
    state: &AppState,
    actor_id: i64,
    username: String,
    display_name: Option<String>,
    now: chrono::DateTime<chrono::Utc>,
    validated: ValidatedCreatePost<'_>,
) -> Response {
    let ValidatedCreatePost {
        text,
        visibility,
        deliver_fedi,
        deliver_bsky,
        recipient_actor_ids,
        reply_to_id_i64,
        quote_of_id_i64,
        quote_notif_recipient,
        poll_json,
        content_warning,
        attachment_ids_i64,
        reply_ctx,
        req,
    } = validated;

    let post_id = generate_snowflake_id(now);
    let ap_object_id = format!("https://{}/notes/{}", state.local_domain, post_id);
    let seiran_post_uuid = uuid::Uuid::new_v4().to_string();

    let (local_emoji_map, response_emojis) = resolve_local_emoji_map(state, &text).await;

    // 引用元情報の取得（Bsky embed / AP quoteUrl を決定する）
    let (bsky_quote_embed, ap_quote) = match quote_of_id_i64 {
        Some(quote_id) => resolve_quote_embed(state, actor_id, quote_id).await,
        None => (None, None),
    };

    // DMのスレッド起点ID。親（reply_to）がdirectならその値をそのまま伝播コピーし、
    // 親がdirectでない/非リプライなら自分自身がスレッド起点になる（再帰クエリで辿らずに
    // 済むよう伝播コピーする）。
    let thread_root_post_id: Option<i64> = if visibility == "direct" {
        Some(reply_ctx.parent_thread_root_post_id.unwrap_or(post_id))
    } else {
        None
    };

    // seiran_post_uuid / reply_to_post_id / quote_of_post_id を含む統合 INSERT
    if let Err(e) = state
        .posts
        .insert_full(InsertFullParams {
            id: post_id,
            actor_id,
            body: &text,
            ap_object_id: &ap_object_id,
            seiran_post_uuid: &seiran_post_uuid,
            reply_to_post_id: reply_to_id_i64,
            quote_of_post_id: quote_of_id_i64,
            created_at: now,
            visibility,
            deliver_fedi,
            deliver_bsky,
            thread_root_post_id,
            recipient_actor_ids: &recipient_actor_ids,
            emoji_map: &local_emoji_map,
            poll: poll_json.as_ref(),
            content_warning: content_warning.as_deref(),
            language: req.language.as_deref(),
        })
        .await
    {
        return ApiError::Internal(format!("投稿の INSERT 失敗: {}", e)).into_response();
    }

    if let Err(e) = attach_media_files(state, post_id, &attachment_ids_i64).await {
        return e.into_response();
    }

    if !req.link_card_urls.is_empty() {
        delivery::attach_link_cards_from_urls(state, post_id, &req.link_card_urls).await;
    }

    if let Err(e) = state.hashtags.link_post(post_id, &text).await {
        tracing::error!(
            "[create_regular_post] ハッシュタグ抽出・リンク失敗（投稿自体は成功済み）: {}",
            e
        );
    }

    // リプライ通知: リプライ先がローカルユーザーの投稿であれば通知を作る（自己リプライは除く）。
    if let Some(parent_actor_id) = reply_ctx.parent_local_actor_id.filter(|id| *id != actor_id) {
        notify_local_actor(
            state,
            parent_actor_id,
            NotificationKind::Reply,
            "reply",
            actor_id,
            post_id,
            &username,
        )
        .await;
    }

    // 引用通知: ローカルユーザーの投稿が他ユーザーに引用された場合に作る。
    if let Some(quoted_actor_id) = quote_notif_recipient {
        notify_local_actor(
            state,
            quoted_actor_id,
            NotificationKind::Quote,
            "quote",
            actor_id,
            post_id,
            &username,
        )
        .await;
    }

    // メンション通知: 本文中で `@username` 形式によりローカルユーザーが言及されていれば通知を
    // 作る。Bsky/AP配送設定の有無とは無関係に常に処理する（配信は宛先プロトコルの話、通知は
    // ローカル受信者の話で別軸のため）。
    // リプライ先本人へのメンションは reply 通知と意味的に重複するため作らない。
    for mentioned_actor_id in
        extract_local_mention_actor_ids(&text, &state.local_domain, &state.db).await
    {
        if mentioned_actor_id == actor_id {
            continue; // 自己メンションは通知しない
        }
        if reply_ctx.parent_local_actor_id == Some(mentioned_actor_id) {
            continue; // reply通知と重複するため
        }
        notify_local_actor(
            state,
            mentioned_actor_id,
            NotificationKind::Mention,
            "mention",
            actor_id,
            post_id,
            &username,
        )
        .await;
    }

    deliver_regular_post(
        state,
        RegularPostDelivery {
            post_id,
            actor_id,
            now,
            text: text.clone(),
            targets: DeliveryTargets {
                fedi: deliver_fedi,
                bsky: deliver_bsky,
            },
            visibility: visibility.to_string(),
            bsky_reply: reply_ctx.bsky_reply,
            bsky_quote_embed,
            ap_quote,
            ap_in_reply_to: reply_ctx.ap_in_reply_to,
            attachment_ids: attachment_ids_i64.clone(),
            bsky_embed_choice: req.bsky_embed_choice.clone(),
            poll: poll_json.clone(),
            content_warning: content_warning.clone(),
            link_card_urls: req.link_card_urls.clone(),
            language: req.language.clone(),
        },
    )
    .await;

    let mut att_map = fetch_attachments_map(&state.db, &[post_id]).await;
    // URLリンクカード（ラジオ選択・チェックボックス選択いずれも上のdeliver_regular_post内/
    // attach_link_cards_from_urlsで既に保存済み）をここでまとめて読み戻す。投稿直後の
    // レスポンス・WebSocketブロードキャストにも反映されるようにするため。
    let mut lc_map = fetch_link_cards_map(&state.db, &[post_id]).await;
    let avatar_url = state
        .actors
        .find_avatar_url(actor_id)
        .await
        .ok()
        .flatten()
        .or_else(|| {
            Some(seiran_common::avatar::fallback_avatar_url(
                &state.local_domain,
                actor_id,
            ))
        });
    let mut note_resp = NoteResponse {
        id: post_id.to_string(),
        text,
        created_at: now.to_rfc3339(),
        user: NoteUserInfo {
            id: actor_id.to_string(),
            username,
            domain: Some(state.local_domain.to_string()),
            display_name,
            actor_type: "local".to_string(),
            avatar_url,
            instance: None,
            follow_status: None,
            is_muted: None,
            is_blocking: None,
            is_blocked_by: None,
            is_repost_muted: None,
        },
        attachments: att_map.remove(&post_id).unwrap_or_default(),
        renote_id: None,
        quote_id: quote_of_id_i64.map(|i| i.to_string()),
        reply_id: reply_to_id_i64.map(|i| i.to_string()),
        renote_status: None,
        quote_status: None,
        reply_status: None,
        parent_original_id: None,
        reactions: vec![],
        renote: None,
        quote: None,
        author_suspended: None, // 投稿作成主体は凍結中に投稿できないため常に非凍結
        reposted_by_me: None,
        emojis: response_emojis,
        pinned_by_me: None,
        visibility: if visibility == "public" {
            None
        } else {
            Some(visibility.to_string())
        },
        thread_root_post_id: thread_root_post_id.map(|i| i.to_string()),
        recipients: None,
        deliver_fedi: Some(deliver_fedi),
        deliver_bsky: Some(deliver_bsky),
        // ローカル投稿なので実際に配送対象とした値そのものが返信可否になる
        // （`notes::delivery::reply_delivery_allowed` と同じ判定基準）。
        reply_fedi_allowed: deliver_fedi,
        reply_bsky_allowed: deliver_bsky,
        reply_blocked: false,
        quote_blocked: false,
        remote_url: None,
        bridge_original_post_id: None,
        content_warning: content_warning.clone(),
        poll: poll_json.clone(),
        reply_count: 0,
        quote_count: 0,
        repost_count: 0,
        link_cards: lc_map.remove(&post_id).unwrap_or_default(),
        content_html: None,
    };
    embed_quotes(
        &state.db,
        std::slice::from_mut(&mut note_resp),
        Some(actor_id),
    )
    .await;
    attach_reply_quote_gates(state, std::slice::from_mut(&mut note_resp), Some(actor_id)).await;
    attach_remote_instance_info(state, std::slice::from_mut(&mut note_resp)).await;
    enqueue_stale_poll_fetches(state, std::slice::from_ref(&note_resp)).await;

    if visibility == "direct" {
        delivery::broadcast_direct_message(state, actor_id, post_id, &note_resp).await;
    } else {
        broadcast_new_note(state, actor_id, &note_resp).await;
    }

    Json(note_resp).into_response()
}

pub async fn create_note(
    user: AuthedUser,
    State(state): State<AppState>,
    Json(req): Json<CreateNoteRequest>,
) -> impl IntoResponse {
    if let Err(e) = user.require_not_did_moved_out() {
        return e.into_response();
    }
    let now = chrono::Utc::now();

    match &req.renote_id {
        Some(renote_id_str) => {
            create_repost(
                &state,
                user.actor_id,
                user.username,
                user.display_name,
                renote_id_str,
                &req,
                now,
            )
            .await
        }
        None => {
            create_regular_post(
                &state,
                user.actor_id,
                user.username,
                user.display_name,
                &req,
                now,
            )
            .await
        }
    }
}

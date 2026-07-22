//! 投稿・リポストを Fedi（ActivityPub）/ Bsky（AT Protocol）へ配送するオーケストレーション。
//! 「何を配送するか」の判断（`classify_post`／配送先制御）と、実際の配送呼び出し
//! （AP は `Job::ApDelivery` へ enqueue、ATP は `AtpCommitService` を直接 await）をまとめる。

use std::collections::HashSet;
use std::sync::Arc;

use seiran_common::atp::{BskyEmbed, BskyPostReply, BskyRefRecord};
use seiran_common::mention::convert_mentions_for_bsky;
use seiran_common::repository::PostDeliveryMeta;
use seiran_common::ApDeliveryKind;

use crate::error::ApiError;
use crate::AppState;

use super::dto::NoteResponse;

/// `at://did/collection/rkey` 形式の AT URI を Bsky.app URL に変換するヘルパー。
pub fn at_uri_to_bsky_app_url(at_uri: &str) -> String {
    let without_prefix = at_uri.strip_prefix("at://").unwrap_or(at_uri);
    let parts: Vec<&str> = without_prefix.splitn(3, '/').collect();
    if parts.len() >= 3 {
        let did = parts[0];
        let rkey = parts[2];
        format!("https://bsky.app/profile/{}/post/{}", did, rkey)
    } else {
        at_uri.to_string()
    }
}

/// ポストの出自（どのプロトコル上に実体を持つか）。配信先の制御に使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostOrigin {
    /// ローカル投稿、または seiran リモート（AP/ATP 両方の実体を持つ）
    LocalOrSeiran,
    /// Fedi リモート（AP 実体のみ）
    FediRemote,
    /// Bsky リモート（ATP 実体のみ）
    BskyRemote,
}

/// 元ポストの種別を判定する。
pub fn classify_post(
    ap_object_id: Option<&str>,
    at_uri: Option<&str>,
    actor_domain: &str,
    local_domain: &str,
) -> PostOrigin {
    // ローカルポストは actors.domain == local_domain
    if actor_domain == local_domain {
        return PostOrigin::LocalOrSeiran;
    }
    match (ap_object_id.is_some(), at_uri.is_some()) {
        // seiran リモート: ap_object_id あり AND at_uri あり（かつ domain != local）
        (true, true) => PostOrigin::LocalOrSeiran,
        // Fedi リモート: ap_object_id あり AND at_uri なし
        (true, false) => PostOrigin::FediRemote,
        // Bsky リモート: ap_object_id なし AND at_uri あり
        (false, true) => PostOrigin::BskyRemote,
        // 判定不能 → ローカル相当として扱う
        (false, false) => PostOrigin::LocalOrSeiran,
    }
}

/// 新規投稿を著者本人 + accepted なローカルフォロワーへ WebSocket でリアルタイム配信する（#37）。
/// `direct`（DM）投稿はこの関数を使わないこと（フォロワーにまで本文が届いてしまう）。
/// 代わりに `broadcast_direct_message` を使う。
pub async fn broadcast_new_note(state: &AppState, actor_id: i64, note: &NoteResponse) {
    let mut recipients: HashSet<i64> = HashSet::new();
    recipients.insert(actor_id);
    if let Ok(rows) = state.follows.find_accepted_local_follower_ids(actor_id).await {
        recipients.extend(rows);
    }
    if let Ok(v) = serde_json::to_value(note) {
        state.stream_hub.publish_note(recipients, &v);
    }
}

/// DM（`visibility='direct'`）投稿を、著者本人 + 宛先（`post_recipients`）のみへ
/// WebSocket でリアルタイム配信する。フォロワーには一切配信しない（本文漏洩防止）。
pub async fn broadcast_direct_message(state: &AppState, actor_id: i64, post_id: i64, note: &NoteResponse) {
    let mut recipients: HashSet<i64> = HashSet::new();
    recipients.insert(actor_id);
    if let Ok(rows) = state.dm.recipient_ids(post_id).await {
        recipients.extend(rows);
    }
    if let Ok(v) = serde_json::to_value(note) {
        state.stream_hub.publish_note(recipients, &v);
    }
}

/// 配信先プロトコルの指定（ユーザーの `deliver_to_*` 指定とリプライ先制約の合成結果）。
#[derive(Clone, Copy)]
pub struct DeliveryTargets {
    pub fedi: bool,
    pub bsky: bool,
}

/// リポストを Fedi（AP Announce）・Bsky（ATP repost）の両プロトコルへ配送する。
/// 元ポストが存在しないプロトコルにはフォールバック（URL テキスト投稿）で代替する。
///
/// AP 側はジョブキュー（Worker）へ積む。ATP 側は firehose ブロードキャストが
/// プロセス内チャネルに結合しているため、Worker 分離まで spawn のまま（レポート A-5）。
pub async fn deliver_repost(
    state: &AppState,
    post_id: i64,
    actor_id: i64,
    now: chrono::DateTime<chrono::Utc>,
    targets: DeliveryTargets,
    meta: &PostDeliveryMeta,
    origin: PostOrigin,
) {
    if targets.fedi {
        if let Some(ref ap_id) = meta.ap_object_id {
            // 元ポストに ap_object_id がある → AP Announce 送信
            state
                .enqueue_ap_delivery(actor_id, ApDeliveryKind::Announce {
                    post_id,
                    original_ap_object_id: ap_id.clone(),
                })
                .await;
        } else if meta.at_uri.is_some() {
            // Bsky リモートポストのリポスト → Fedi フォールバック: URL テキスト投稿
            let bsky_url = at_uri_to_bsky_app_url(meta.at_uri.as_deref().unwrap_or(""));
            let author_name = meta.display_name.as_deref().unwrap_or(&meta.username).to_string();
            let fallback_text = format!("🔁 {}: {}", author_name, bsky_url);
            state
                .enqueue_ap_delivery(actor_id, ApDeliveryKind::PostToFollowers {
                    post_id,
                    body: Some(fallback_text),
                    quote_url: None,
                    in_reply_to: None,
                })
                .await;
        }
    }

    // 二重防御: 元ポストが followers_only/direct（＝本来 create_repost で弾かれているはず）
    // なら Bsky コミットを行わない。呼び出し元の実装ミスに対する最終ガードとして再チェックする。
    let bsky_target = targets.bsky && meta.visibility != "followers_only" && meta.visibility != "direct";
    if targets.bsky && !bsky_target {
        tracing::warn!(
            "[deliver_repost] visibility={} のポストへの Bsky リポストが要求されたためスキップ（呼び出し元のバグの可能性、post_id={}）",
            meta.visibility, post_id
        );
    }

    if bsky_target {
        if let (Some(at_uri), Some(at_cid)) = (&meta.at_uri, &meta.at_cid) {
            // 元ポストに at_uri と at_cid がある → ATP repost
            let at_uri_clone = at_uri.clone();
            let at_cid_clone = at_cid.clone();
            let atp = Arc::clone(&state.atp_service);
            tokio::spawn(async move {
                if let Err(e) = atp.commit_repost(actor_id, &at_uri_clone, &at_cid_clone, now, Some(post_id)).await {
                    tracing::error!("[create_note] ATP repost 失敗: {}", e);
                }
            });
        } else if origin != PostOrigin::BskyRemote && meta.ap_object_id.is_some() {
            // at_uri なし（Fedi リモートまたはローカル）→ Bsky フォールバック: URL テキスト投稿
            // リポストラッパー行（post_id）自体を PDS 上のテキストポストとしてコミットする。
            // commit_post に post_id を渡すことで posts.at_uri/at_cid/at_rkey がこの行に
            // 書き込まれ、自前 Jetstream の自己エコー（save_bsky_post）が
            // `ON CONFLICT (at_uri) DO NOTHING` により重複ポストを作らなくなる
            // （このリポストと無関係な別ノートがタイムラインに現れなくなる）。
            let ap_id = meta.ap_object_id.clone().unwrap_or_default();
            let author_name = meta.display_name.as_deref().unwrap_or(&meta.username).to_string();
            let fallback_text = format!("🔁 {}: {}", author_name, ap_id);
            let atp = Arc::clone(&state.atp_service);
            tokio::spawn(async move {
                if let Err(e) = atp.commit_post(actor_id, post_id, &fallback_text, vec![], &[], now, None).await {
                    tracing::error!("[create_note] Fedi→Bsky フォールバック投稿失敗: {}", e);
                }
            });
        }
    }
}

/// リプライ先の配信先制御・可視性継承に使う情報。
pub struct ReplyContext {
    pub deliver_fedi_allowed: bool,
    pub deliver_bsky_allowed: bool,
    pub bsky_reply: Option<BskyPostReply>,
    pub ap_in_reply_to: Option<String>,
    /// 親ポストの可視性（非リプライの場合は `None`）。
    /// "public"/"unlisted"/"followers_only"/"direct" のいずれか。
    pub parent_visibility: Option<String>,
    /// 親ポストが`direct`（DM）の場合のスレッド起点ポストID。DM返信時、この値を
    /// そのまま子ポストへ伝播コピーする（親が`direct`でなければ`None`）。
    pub parent_thread_root_post_id: Option<i64>,
    /// 親ポストの投稿者がローカルユーザーの場合のみ、その actor_id（リプライ通知の宛先に使う）。
    pub parent_local_actor_id: Option<i64>,
}

impl ReplyContext {
    /// リプライ先の可視性制約を踏まえて、リクエストされた visibility を確定する。
    /// - 親が`direct`（DMスレッド内の返信）: 常に`direct`を強制する（往復の途中で
    ///   他の可視性へ離脱させない）。
    /// - 非リプライ、または親が public: 制約なし（従来のバリデーション、デフォルト public）。
    ///   ただし`direct`が明示指定されれば許可する（通常ポストへの返信として新規DMを開始する経路）。
    /// - 親が followers_only: 強制的に followers_only（Misskey互換の黙った読み替え）。
    /// - 親が unlisted: public/unlisted/followers_only いずれも選択可、デフォルトは unlisted。
    pub fn resolve_visibility(&self, requested: Option<&str>) -> Result<&'static str, ApiError> {
        match self.parent_visibility.as_deref() {
            Some("direct") => Ok("direct"),
            Some("followers_only") => Ok("followers_only"),
            Some("unlisted") => match requested {
                None | Some("unlisted") => Ok("unlisted"),
                Some("public") => Ok("public"),
                Some("followers_only") => Ok("followers_only"),
                Some(_) => Err(ApiError::BadRequest("INVALID_VISIBILITY".to_owned())),
            },
            // 非リプライ、または親が public/未知値 → 従来ロジック
            _ => match requested {
                None | Some("public") => Ok("public"),
                Some("unlisted") => Ok("unlisted"),
                Some("followers_only") => Ok("followers_only"),
                Some("direct") => Ok("direct"),
                Some(_) => Err(ApiError::BadRequest("INVALID_VISIBILITY".to_owned())),
            },
        }
    }
}

/// リプライ先ポストの種別を判定し、配信先制御（元ポストが存在しないプロトコルには配信しない）と
/// ATP reply フィールドを組み立てる。`viewer_actor_id` はリプライしようとしている本人で、
/// リプライ先の投稿者とブロック関係にある場合はリプライ自体を拒否する（Bsky準拠のブロック定義）。
pub async fn resolve_reply_context(state: &AppState, reply_to_id_str: &str, viewer_actor_id: i64) -> Result<ReplyContext, ApiError> {
    let reply_to_id: i64 = reply_to_id_str
        .parse()
        .map_err(|_| ApiError::BadRequest("INVALID_REPLY_TO_ID".to_owned()))?;

    let meta = state
        .posts
        .find_delivery_meta(reply_to_id)
        .await
        .map_err(|e| ApiError::Internal(format!("reply 元ポスト取得失敗: {}", e)))?
        .ok_or(ApiError::NotFound("REPLY_TARGET_NOT_FOUND"))?;

    crate::handlers::target_resolve::check_not_blocked(state, viewer_actor_id, meta.actor_id).await?;

    let origin = classify_post(
        meta.ap_object_id.as_deref(),
        meta.at_uri.as_deref(),
        &meta.domain,
        &state.local_domain,
    );

    // 配信先制御: 元ポストが存在しないプロトコルには配信しない
    let deliver_fedi_allowed = origin != PostOrigin::BskyRemote; // Bsky リモートへのリプライ → Fedi 配信しない
    let deliver_bsky_allowed = origin != PostOrigin::FediRemote; // Fedi リモートへのリプライ → Bsky 配信しない

    // ATP reply フィールド: Bsky 配信する場合かつ at_uri/at_cid が取得できる場合のみ設定
    let bsky_reply = if deliver_bsky_allowed {
        match (&meta.at_uri, &meta.at_cid) {
            (Some(uri), Some(cid)) => Some(BskyPostReply {
                root: BskyRefRecord { cid: cid.clone(), uri: uri.clone() },
                parent: BskyRefRecord { cid: cid.clone(), uri: uri.clone() },
            }),
            _ => None,
        }
    } else {
        None
    };

    let parent_local_actor_id = if meta.domain == state.local_domain { Some(meta.actor_id) } else { None };

    Ok(ReplyContext {
        deliver_fedi_allowed,
        deliver_bsky_allowed,
        bsky_reply,
        ap_in_reply_to: meta.ap_object_id,
        parent_visibility: Some(meta.visibility.clone()),
        parent_thread_root_post_id: meta.thread_root_post_id,
        parent_local_actor_id,
    })
}

/// 引用元ポストの種別から Bsky embed（引用埋め込み）と AP quoteUrl を組み立てる。
pub async fn resolve_quote_embed(state: &AppState, quote_of_id: i64) -> (Option<BskyEmbed>, Option<String>) {
    let meta = match state.posts.find_delivery_meta(quote_of_id).await {
        Ok(Some(m)) => m,
        _ => return (None, None),
    };

    let origin = classify_post(
        meta.ap_object_id.as_deref(), meta.at_uri.as_deref(), &meta.domain, &state.local_domain,
    );

    let bsky_embed = if origin == PostOrigin::FediRemote {
        meta.ap_object_id.as_deref().map(|u| BskyEmbed::External { url: u.to_string() })
    } else if let (Some(uri), Some(cid)) = (&meta.at_uri, &meta.at_cid) {
        Some(BskyEmbed::Record { uri: uri.clone(), cid: cid.clone() })
    } else {
        meta.ap_object_id.as_deref().map(|u| BskyEmbed::External { url: u.to_string() })
    };

    let ap_url = if meta.at_uri.is_some() && meta.ap_object_id.is_none() {
        meta.at_uri.as_deref().map(at_uri_to_bsky_app_url)
    } else {
        meta.ap_object_id.clone()
    };

    (bsky_embed, ap_url)
}

/// 通常投稿 / リプライ / 引用投稿の配送指示。
pub struct RegularPostDelivery {
    pub post_id: i64,
    pub actor_id: i64,
    pub now: chrono::DateTime<chrono::Utc>,
    pub text: String,
    pub targets: DeliveryTargets,
    /// 投稿の可視性（"public" | "unlisted" | "followers_only"）。Bsky はプロトコル上
    /// followers_only 配信をサポートしないため、その場合は Bsky コミットをスキップする
    /// 最終防御に使う（unlisted は Bsky 配送可能）。
    pub visibility: String,
    pub bsky_reply: Option<BskyPostReply>,
    pub bsky_quote_embed: Option<BskyEmbed>,
    pub ap_quote_url: Option<String>,
    pub ap_in_reply_to: Option<String>,
    pub attachment_ids: Vec<i64>,
}

/// `attachment_ids` の中に、Bsky 動画パイプライン結合がまだ確定状態
/// （`ready`/`failed`）に達していない動画添付が1件でもあるか判定する。
async fn has_pending_video(pool: &sqlx::PgPool, attachment_ids: &[i64]) -> bool {
    if attachment_ids.is_empty() {
        return false;
    }
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM media_files
         WHERE id = ANY($1) AND mime_type LIKE 'video/%'
           AND bsky_video_status IS DISTINCT FROM 'ready'
           AND bsky_video_status IS DISTINCT FROM 'failed'",
    )
    .bind(attachment_ids)
    .fetch_one(pool)
    .await
    .map(|c| c > 0)
    .unwrap_or(false)
}

/// ATP レコードの `(uri, cid)` 参照。
type AtUriCid = (String, String);

/// `BskyPostReply` を `Job::BskyPostCommitDeferred` へ渡せる `(uri, cid)` タプルに分解する。
fn split_bsky_reply(reply: &Option<BskyPostReply>) -> (Option<AtUriCid>, Option<AtUriCid>) {
    match reply {
        Some(r) => (
            Some((r.root.uri.clone(), r.root.cid.clone())),
            Some((r.parent.uri.clone(), r.parent.cid.clone())),
        ),
        None => (None, None),
    }
}

/// 通常投稿 / リプライ / 引用投稿を Fedi・Bsky へ配送する。
/// Bsky は ATP コミット（firehose 結合のため in-process）、Fedi は ApDelivery ジョブ。
pub async fn deliver_regular_post(state: &AppState, d: RegularPostDelivery) {
    if d.visibility == "direct" {
        // DM: Fedi宛先へは`DirectMessage`ジョブ（post_recipientsからFediアクターを解決）、
        // Bsky宛先へは`BskyDmSend`ジョブ（chat.bsky.convo.sendMessage）でそれぞれ配送する。
        if d.targets.fedi {
            state.enqueue_ap_delivery(d.actor_id, ApDeliveryKind::DirectMessage { post_id: d.post_id }).await;
        }
        if d.targets.bsky {
            state.enqueue_bsky_dm_send(d.post_id).await;
        }
        return;
    }

    // 二重防御: visibility が followers_only なら Bsky コミットを行わない（Bsky はプロトコル上
    // フォロワー限定配信をサポートしないため）。create_regular_post 側で既に deliver_bsky を
    // false に読み替え済みのはずだが、呼び出し元の実装ミスに対する最終ガードとして再チェックする。
    let bsky_target = d.targets.bsky && d.visibility != "followers_only";
    if d.targets.bsky && !bsky_target {
        tracing::warn!(
            "[deliver_regular_post] visibility={} で Bsky 配送が要求されたためスキップ（呼び出し元のバグの可能性、post_id={}）",
            d.visibility, d.post_id
        );
    }

    // 動画添付があり、まだ Bsky 動画パイプライン結合（トランスコード）が確定していない場合、
    // ここで即座に commit_post すると常に app.bsky.embed.external にフォールバックしてしまう
    // （一度 external でコミットされた投稿は再コミットされないため、以後 video embed 化
    // されることもない）。投稿ボタンを押すタイミングが早すぎるだけで起きる問題なので、
    // Bsky コミット自体を Worker（Job::BskyPostCommitDeferred）に委譲し、結合完了を
    // 待ってからコミットする（2026-07-17 マイケル指摘・実機再現確認。引用投稿は対象外）。
    let defer_for_video = bsky_target
        && d.bsky_quote_embed.is_none()
        && has_pending_video(&state.db, &d.attachment_ids).await;

    if defer_for_video {
        let (reply_root, reply_parent) = split_bsky_reply(&d.bsky_reply);
        state
            .enqueue_bsky_post_commit_deferred(
                d.actor_id,
                d.post_id,
                d.text.clone(),
                d.attachment_ids.clone(),
                reply_root,
                reply_parent,
                d.now,
            )
            .await;
    } else if bsky_target {
        // メンション変換（変換失敗時は元テキストをそのまま使用する）
        // Bsky 配信用: `@username` → `@username.{local_domain}`、`@user@domain` → brid.gy ハンドル
        let (bsky_text, bsky_facets) =
            convert_mentions_for_bsky(&d.text, &state.local_domain, &state.db, state.ap_client.http.as_ref()).await;

        if let Some(embed) = d.bsky_quote_embed {
            // 引用投稿: embed を付けて commit_quote を使う（画像 embed と共存しない）
            if let Err(e) = state.atp_service.commit_quote(d.actor_id, d.post_id, &bsky_text, bsky_facets, Some(embed), d.now, d.bsky_reply).await {
                tracing::error!("[create_note] ATP quote commit 失敗（投稿は保存済み）: {}", e);
            }
        } else if let Err(e) = state.atp_service.commit_post(d.actor_id, d.post_id, &bsky_text, bsky_facets, &d.attachment_ids, d.now, d.bsky_reply).await {
            tracing::error!("[create_note] ATP コミット失敗（投稿は保存済み）: {}", e);
        }
    }

    if d.targets.fedi {
        // body は渡さない。deliver_post_to_ap_followers 側で DB の投稿本文を取得し、
        // メンション解決（tag[]・<a> アンカー付与）まで一貫して行う。
        state
            .enqueue_ap_delivery(d.actor_id, ApDeliveryKind::PostToFollowers {
                post_id: d.post_id,
                body: None,
                quote_url: d.ap_quote_url,
                in_reply_to: d.ap_in_reply_to,
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::{at_uri_to_bsky_app_url, classify_post, PostOrigin, ReplyContext};
    use crate::error::ApiError;

    fn ctx_with_parent_visibility(parent_visibility: Option<&str>) -> ReplyContext {
        ReplyContext {
            deliver_fedi_allowed: true,
            deliver_bsky_allowed: true,
            bsky_reply: None,
            ap_in_reply_to: None,
            parent_visibility: parent_visibility.map(str::to_owned),
            parent_thread_root_post_id: None,
            parent_local_actor_id: None,
        }
    }

    // ─── resolve_visibility ────────────────────────────────────────────────
    // 可視性継承ロジック（間違えるとDM/フォロワー限定投稿が意図せず公開される情報漏洩に
    // 直結するため、親visibility×requestedの主要な組み合わせを網羅する）。

    #[test]
    fn resolve_visibility_non_reply_defaults_to_public() {
        let ctx = ctx_with_parent_visibility(None);
        assert_eq!(ctx.resolve_visibility(None).unwrap(), "public");
    }

    #[test]
    fn resolve_visibility_non_reply_allows_starting_a_dm() {
        // 通常ポストへの返信として新規DMを開始する経路。
        let ctx = ctx_with_parent_visibility(None);
        assert_eq!(ctx.resolve_visibility(Some("direct")).unwrap(), "direct");
    }

    #[test]
    fn resolve_visibility_direct_parent_always_forces_direct() {
        let ctx = ctx_with_parent_visibility(Some("direct"));
        assert_eq!(ctx.resolve_visibility(None).unwrap(), "direct");
        // 明示的にpublicを指定してもDMスレッドから離脱させない。
        assert_eq!(ctx.resolve_visibility(Some("public")).unwrap(), "direct");
        assert_eq!(ctx.resolve_visibility(Some("followers_only")).unwrap(), "direct");
    }

    #[test]
    fn resolve_visibility_followers_only_parent_forces_followers_only() {
        let ctx = ctx_with_parent_visibility(Some("followers_only"));
        assert_eq!(ctx.resolve_visibility(None).unwrap(), "followers_only");
        assert_eq!(ctx.resolve_visibility(Some("public")).unwrap(), "followers_only");
    }

    #[test]
    fn resolve_visibility_unlisted_parent_defaults_to_unlisted() {
        let ctx = ctx_with_parent_visibility(Some("unlisted"));
        assert_eq!(ctx.resolve_visibility(None).unwrap(), "unlisted");
    }

    #[test]
    fn resolve_visibility_unlisted_parent_allows_public_or_followers_only() {
        let ctx = ctx_with_parent_visibility(Some("unlisted"));
        assert_eq!(ctx.resolve_visibility(Some("public")).unwrap(), "public");
        assert_eq!(ctx.resolve_visibility(Some("followers_only")).unwrap(), "followers_only");
        assert_eq!(ctx.resolve_visibility(Some("unlisted")).unwrap(), "unlisted");
    }

    #[test]
    fn resolve_visibility_unlisted_parent_rejects_direct() {
        let ctx = ctx_with_parent_visibility(Some("unlisted"));
        assert!(matches!(ctx.resolve_visibility(Some("direct")), Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn resolve_visibility_public_parent_allows_any_valid_value() {
        let ctx = ctx_with_parent_visibility(Some("public"));
        assert_eq!(ctx.resolve_visibility(Some("unlisted")).unwrap(), "unlisted");
        assert_eq!(ctx.resolve_visibility(Some("followers_only")).unwrap(), "followers_only");
        assert_eq!(ctx.resolve_visibility(Some("direct")).unwrap(), "direct");
    }

    #[test]
    fn resolve_visibility_rejects_unknown_value() {
        let ctx = ctx_with_parent_visibility(None);
        assert!(matches!(ctx.resolve_visibility(Some("bogus")), Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn at_uri_to_bsky_app_url_valid() {
        assert_eq!(
            at_uri_to_bsky_app_url("at://did:plc:abc123/app.bsky.feed.post/xyz789"),
            "https://bsky.app/profile/did:plc:abc123/post/xyz789"
        );
    }

    #[test]
    fn at_uri_to_bsky_app_url_missing_prefix_passthrough() {
        // "at://" プレフィックスがない・パーツ不足の場合はそのまま返す
        assert_eq!(at_uri_to_bsky_app_url("not-an-at-uri"), "not-an-at-uri");
        assert_eq!(at_uri_to_bsky_app_url("at://did:plc:abc123"), "at://did:plc:abc123");
    }

    #[test]
    fn classify_post_local_domain_match() {
        // domain が local_domain と一致する場合は ap_object_id / at_uri の値によらずローカル扱い
        assert_eq!(
            classify_post(None, None, "seiran.example", "seiran.example"),
            PostOrigin::LocalOrSeiran
        );
    }

    #[test]
    fn classify_post_seiran_remote_has_both_ids() {
        assert_eq!(
            classify_post(Some("https://a/notes/1"), Some("at://did/x/y"), "other.example", "seiran.example"),
            PostOrigin::LocalOrSeiran
        );
    }

    #[test]
    fn classify_post_fedi_remote_ap_only() {
        assert_eq!(
            classify_post(Some("https://mastodon.example/notes/1"), None, "mastodon.example", "seiran.example"),
            PostOrigin::FediRemote
        );
    }

    #[test]
    fn classify_post_bsky_remote_at_uri_only() {
        assert_eq!(
            classify_post(None, Some("at://did/x/y"), "bsky.example", "seiran.example"),
            PostOrigin::BskyRemote
        );
    }

    #[test]
    fn classify_post_unknown_defaults_to_local() {
        assert_eq!(
            classify_post(None, None, "other.example", "seiran.example"),
            PostOrigin::LocalOrSeiran
        );
    }
}

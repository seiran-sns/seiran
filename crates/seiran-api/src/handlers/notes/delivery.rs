//! 投稿・リポストを Fedi（ActivityPub）/ Bsky（AT Protocol）へ配送するオーケストレーション。
//! 「何を配送するか」の判断（`classify_post`／配送先制御）と、実際の配送呼び出し
//! （AP は `Job::ApDelivery` へ enqueue、ATP は `AtpCommitService` を直接 await）をまとめる。

use std::collections::HashSet;
use std::sync::Arc;

use seiran_common::atp::{cid_from_sha256_hex, BskyEmbed, BskyImage, BskyPostReply, BskyRefRecord};
use seiran_common::mention::convert_mentions_for_bsky;
use seiran_common::net::{extract_body_urls, fetch_ogp};
use seiran_common::repository::PostDeliveryMeta;
use seiran_common::{prepare_image, ApDeliveryKind, MediaKind, AUDIO_VIDEO_HEIGHT, AUDIO_VIDEO_WIDTH};

use crate::error::ApiError;
use crate::AppState;

use super::dto::{BskyEmbedChoice, NoteResponse};

const BSKY_CARD_THUMB_MAX_BYTES: u64 = 20 * 1024 * 1024;

async fn prepare_external_thumb(
    state: &AppState,
    actor_id: i64,
    url: Option<&str>,
) -> Option<BskyImage> {
    let url = url?;
    let response = match state.ap_client.http.get(url).send().await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::warn!(
                "[deliver_repost] URLカード画像取得失敗 status={} url={}",
                response.status(),
                url
            );
            return None;
        }
        Err(error) => {
            tracing::warn!(
                "[deliver_repost] URLカード画像取得失敗: {} url={}",
                error,
                url
            );
            return None;
        }
    };
    if response
        .content_length()
        .is_some_and(|size| size > BSKY_CARD_THUMB_MAX_BYTES)
    {
        tracing::warn!("[deliver_repost] URLカード画像が上限超過 url={}", url);
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    if bytes.len() as u64 > BSKY_CARD_THUMB_MAX_BYTES {
        tracing::warn!("[deliver_repost] URLカード画像が上限超過 url={}", url);
        return None;
    }
    let pipeline = prepare_image(&bytes, MediaKind::Post).ok()?;
    let stored = super::super::media_store::store_image(state, pipeline, Some(actor_id))
        .await
        .ok()?;
    Some(BskyImage {
        sha256_hex: stored.record.sha256,
        mime_type: stored.record.mime_type,
        size: stored.record.size,
        width: stored.record.width.unwrap_or(0),
        height: stored.record.height.unwrap_or(0),
        alt: String::new(),
    })
}

async fn build_external_post_embed(
    state: &AppState,
    actor_id: i64,
    meta: &PostDeliveryMeta,
) -> Option<BskyEmbed> {
    let url = meta.ap_object_id.as_ref()?;
    let title = format!(
        "{} (@{}@{})",
        meta.display_name.as_deref().unwrap_or(&meta.username),
        meta.username,
        meta.domain
    );
    let thumb_url = meta
        .first_image_url
        .as_deref()
        .or(meta.avatar_url.as_deref());
    let thumb = prepare_external_thumb(state, actor_id, thumb_url).await;
    Some(BskyEmbed::External {
        url: url.clone(),
        title,
        description: meta.body.clone(),
        thumb,
    })
}

/// Bsky embed候補の分類に使う `media_files` 行（#227 Bsky embed選択）。
#[derive(sqlx::FromRow)]
struct EmbedCandidateRow {
    id: i64,
    sha256: String,
    size: i64,
    mime_type: String,
    width: Option<i32>,
    height: Option<i32>,
    is_animated_image: bool,
    bsky_video_cid: Option<String>,
    bsky_video_status: Option<String>,
    bsky_video_size: Option<i64>,
}

/// [`resolve_bsky_embed`] の結果。
pub enum BskyEmbedResolution {
    /// 即座にcommitできる（`None`は「候補なし」＝embed無しで投稿）。
    Ready(Option<BskyEmbed>),
    /// 選択された添付が Bsky 動画パイプライン結合未確定のため、確定を待つ必要がある
    /// （`Job::BskyPostCommitDeferred` へ委譲、対象は `media_files.id`）。
    Pending(i64),
}

fn watch_page_fallback_embed(state: &AppState, media_file_id: i64) -> BskyEmbed {
    // 音声（Bskyに専用embedが無い）・動画パイプライン未完了/失敗時のフォールバックリンク先は、
    // メディアファイルの直リンクではなく簡易視聴ページ（`handlers::drive::watch_media`）にする。
    // 直リンクだとブラウザがダウンロードしてしまい再生できないため（2026-07-17 マイケル指摘）。
    BskyEmbed::External {
        url: format!(
            "https://{}/api/media/{}/watch",
            state.local_domain, media_file_id
        ),
        title: String::new(),
        description: String::new(),
        thumb: None,
    }
}

fn to_bsky_image(row: &EmbedCandidateRow) -> Option<BskyImage> {
    // CID 生成に失敗したものはスキップ
    cid_from_sha256_hex(&row.sha256).ok()?;
    Some(BskyImage {
        sha256_hex: row.sha256.clone(),
        mime_type: row.mime_type.clone(),
        size: row.size,
        width: row.width.unwrap_or(0),
        height: row.height.unwrap_or(0),
        alt: String::new(),
    })
}

/// 1件の添付行をBsky embedへ変換する（`Attachment{id}`選択・優先順位フォールバックの
/// 動画/音声/アニメGIF枠の共通処理）。画像（アニメGIF含む）は単独の`Images`embed、
/// 動画/音声はパイプライン状態に応じて`Video`/視聴ページへの`External`フォールバック、
/// もしくは未確定として`Pending`を返す。
fn resolve_attachment_embed(state: &AppState, row: &EmbedCandidateRow) -> BskyEmbedResolution {
    if row.mime_type.starts_with("image/") {
        return BskyEmbedResolution::Ready(to_bsky_image(row).map(|img| BskyEmbed::Images(vec![img])));
    }
    match row.bsky_video_status.as_deref() {
        Some("ready") => match &row.bsky_video_cid {
            Some(video_cid) => {
                let is_audio = row.mime_type.starts_with("audio/");
                let (width, height) = if is_audio {
                    (AUDIO_VIDEO_WIDTH as i32, AUDIO_VIDEO_HEIGHT as i32)
                } else {
                    (row.width.unwrap_or(0), row.height.unwrap_or(0))
                };
                BskyEmbedResolution::Ready(Some(BskyEmbed::Video {
                    cid: video_cid.clone(),
                    mime_type: "video/mp4".to_string(),
                    size: row.bsky_video_size.unwrap_or(row.size),
                    width,
                    height,
                }))
            }
            None => BskyEmbedResolution::Ready(Some(watch_page_fallback_embed(state, row.id))),
        },
        Some("failed") => BskyEmbedResolution::Ready(Some(watch_page_fallback_embed(state, row.id))),
        _ => BskyEmbedResolution::Pending(row.id),
    }
}

/// 本文中の特定URLをBsky embed選択した場合の `External` embed を組み立てる。OGP
/// （title/description/thumbnail）を同期取得し、取得できてもできなくても選択自体は常に
/// 尊重する（取得失敗時は素の `External`）。あわせて、seiranローカルでも同じURLをカード表示
/// できるよう `post_link_cards`（position=0）へ保存する（マイケル指摘。選択が無ければ
/// ローカル表示にカードが出ないのは「選んで初めてカード化する」という仕様のため）。
async fn resolve_url_embed(state: &AppState, actor_id: i64, post_id: i64, url: String) -> BskyEmbed {
    let ogp = fetch_ogp(&url).await.ok().flatten();
    let title = ogp.as_ref().map(|o| o.title.clone()).unwrap_or_default();
    let description = ogp
        .as_ref()
        .map(|o| o.description.clone())
        .unwrap_or_default();
    let thumbnail_url = ogp.as_ref().and_then(|o| o.thumbnail_url.clone());
    let thumb = prepare_external_thumb(state, actor_id, thumbnail_url.as_deref()).await;

    if let Err(e) = sqlx::query(
        "INSERT INTO post_link_cards (post_id, position, url, title, description, thumbnail_url)
         VALUES ($1, 0, $2, $3, $4, $5)",
    )
    .bind(post_id)
    .bind(&url)
    .bind(&title)
    .bind(&description)
    .bind(&thumbnail_url)
    .execute(&state.db)
    .await
    {
        tracing::warn!(
            "[resolve_url_embed] post_link_cards INSERT失敗（Bsky embed自体は継続） post_id={} err={}",
            post_id, e
        );
    }

    BskyEmbed::External {
        url,
        title,
        description,
        thumb,
    }
}

/// アンケート（#228）をBsky embed選択した場合の `External` embed を組み立てる。このポスト
/// 自身のURLを、選択肢名だけの箇条書きプレーンテキスト（`- 選択肢A\n- 選択肢B`）を
/// descriptionにしてリンクカード化する。投稿の言語が決定できないため見出し文・案内文は
/// 付けない。作成時点の得票は常に0で、Bsky embedは一度コミットすると再コミットされず
/// 得票を反映できないため、得票バー・パーセンテージ表示も行わない（マイケル指摘）。
/// `post_link_cards`へのINSERTは行わない（このポスト自身が`NoteResponse.poll`経由で既に
/// リッチなアンケートUIを表示するため、自分自身を指すリンクカードを重ねて表示するのは
/// 冗長・表示上不自然なため）。
fn resolve_poll_embed(local_domain: &str, post_id: i64, poll: &serde_json::Value) -> BskyEmbed {
    let description = poll["options"]
        .as_array()
        .map(|options| {
            options
                .iter()
                .filter_map(|o| o["name"].as_str())
                .map(|name| format!("- {name}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    BskyEmbed::External {
        url: format!("https://{}/notes/{}", local_domain, post_id),
        title: String::new(),
        description,
        thumb: None,
    }
}

/// Bsky配送するローカル投稿の embed を、明示選択（`CreateNoteRequest::bsky_embed_choice`）
/// または省略時の固定優先順位（アンケート > 静止画 > アニメGIF先頭 > 動画/音声先頭 >
/// 本文URL先頭）から解決する（#227、アンケートは#228で追加）。
pub async fn resolve_bsky_embed(
    state: &AppState,
    actor_id: i64,
    post_id: i64,
    attachment_ids: &[i64],
    body_text: &str,
    poll: Option<&serde_json::Value>,
    choice: Option<BskyEmbedChoice>,
) -> BskyEmbedResolution {
    let rows: Vec<EmbedCandidateRow> = if attachment_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as::<_, EmbedCandidateRow>(
            "SELECT id, sha256, size, mime_type, width, height, is_animated_image, \
                    bsky_video_cid, bsky_video_status, bsky_video_size \
             FROM media_files WHERE id = ANY($1) ORDER BY array_position($1, id)",
        )
        .bind(attachment_ids)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
    };

    enum Target<'a> {
        Poll,
        Images,
        Attachment(&'a EmbedCandidateRow),
        Url(String),
        None,
    }

    let target = match choice {
        Some(BskyEmbedChoice::Poll) => {
            if poll.is_some() {
                Target::Poll
            } else {
                Target::None
            }
        }
        Some(BskyEmbedChoice::Images) => Target::Images,
        Some(BskyEmbedChoice::Attachment { id }) => match id.parse::<i64>() {
            Ok(id) => rows
                .iter()
                .find(|r| r.id == id)
                .map(Target::Attachment)
                .unwrap_or(Target::None),
            Err(_) => Target::None,
        },
        Some(BskyEmbedChoice::Url { url }) => Target::Url(url),
        None => {
            if poll.is_some() {
                Target::Poll
            } else if rows
                .iter()
                .any(|r| r.mime_type.starts_with("image/") && !r.is_animated_image)
            {
                Target::Images
            } else if let Some(row) = rows.iter().find(|r| r.is_animated_image) {
                Target::Attachment(row)
            } else if let Some(row) = rows
                .iter()
                .find(|r| r.mime_type.starts_with("video/") || r.mime_type.starts_with("audio/"))
            {
                Target::Attachment(row)
            } else if let Some(url) = extract_body_urls(body_text).into_iter().next() {
                Target::Url(url)
            } else {
                Target::None
            }
        }
    };

    match target {
        Target::None => BskyEmbedResolution::Ready(None),
        Target::Poll => {
            // `Target::Poll`はchoiceがPollかつpollがSomeの場合、またはNone（自動選択）で
            // pollがSomeの場合のみ到達するため、ここでのunwrapは安全。
            BskyEmbedResolution::Ready(Some(resolve_poll_embed(
                &state.local_domain,
                post_id,
                poll.expect("Target::Poll implies poll.is_some()"),
            )))
        }
        Target::Images => {
            let images: Vec<BskyImage> = rows
                .iter()
                .filter(|r| r.mime_type.starts_with("image/") && !r.is_animated_image)
                .filter_map(to_bsky_image)
                // app.bsky.embed.images の上限は4枚（AT Protocol仕様）。ポスト自体は最大10枚
                // まで許容するが、Bsky embedには先頭4枚のみ含める。
                .take(4)
                .collect();
            BskyEmbedResolution::Ready((!images.is_empty()).then_some(BskyEmbed::Images(images)))
        }
        Target::Attachment(row) => resolve_attachment_embed(state, row),
        Target::Url(url) => {
            BskyEmbedResolution::Ready(Some(resolve_url_embed(state, actor_id, post_id, url).await))
        }
    }
}

pub use seiran_common::ap::deliver::at_uri_to_bsky_app_url;

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

/// 元ポストの種別を判定する。`is_local`は`actors.actor_type == "local"`（呼び出し元は
/// `PostDeliveryMeta::actor_type`から渡す）。
pub fn classify_post(
    ap_object_id: Option<&str>,
    at_uri: Option<&str>,
    is_local: bool,
) -> PostOrigin {
    if is_local {
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

/// 新規投稿を著者本人 + accepted なローカルフォロワーへ、購読中のタイムラインチャンネル
/// （homeTimeline/localTimeline/hybridTimeline/globalTimeline/userList/hashtag）へ
/// WebSocket でリアルタイム配信する（#37）。
/// `direct`（DM）投稿はこの関数を使わないこと（フォロワーにまで本文が届いてしまう）。
/// 代わりに `broadcast_direct_message` を使う。
pub async fn broadcast_new_note(state: &AppState, actor_id: i64, note: &NoteResponse) {
    let mut home_recipients: HashSet<i64> = HashSet::new();
    home_recipients.insert(actor_id);
    if let Ok(rows) = state
        .follows
        .find_accepted_local_follower_ids(actor_id)
        .await
    {
        home_recipients.extend(rows);
    }
    let list_ids: HashSet<i64> = state
        .lists
        .list_ids_containing_actor(actor_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    let hashtags: HashSet<String> = seiran_common::hashtag::extract_hashtags(&note.text)
        .into_iter()
        .collect();
    let scope = seiran_common::streaming::ChannelScope {
        is_local: true,
        visibility: note
            .visibility
            .clone()
            .unwrap_or_else(|| "public".to_string()),
        home_recipients: Arc::new(home_recipients),
        list_ids: Arc::new(list_ids),
        hashtags: Arc::new(hashtags),
    };
    if let Ok(v) = serde_json::to_value(note) {
        state.stream_hub.publish_channel_note(scope, v);
    }
}

/// DM（`visibility='direct'`）投稿を、著者本人 + 宛先（`post_recipients`）のみへ
/// WebSocket でリアルタイム配信する。フォロワーには一切配信しない（本文漏洩防止）。
pub async fn broadcast_direct_message(
    state: &AppState,
    actor_id: i64,
    post_id: i64,
    note: &NoteResponse,
) {
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
                .enqueue_ap_delivery(
                    actor_id,
                    ApDeliveryKind::Announce {
                        post_id,
                        original_ap_object_id: ap_id.clone(),
                    },
                )
                .await;
        } else if meta.at_uri.is_some() {
            // Bsky リモートポストのリポスト → Fedi フォールバック: URL テキスト投稿
            let bsky_url = at_uri_to_bsky_app_url(meta.at_uri.as_deref().unwrap_or(""));
            let author_name = meta
                .display_name
                .as_deref()
                .unwrap_or(&meta.username)
                .to_string();
            let fallback_text = format!("🔁 {}: {}", author_name, bsky_url);
            state
                .enqueue_ap_delivery(
                    actor_id,
                    ApDeliveryKind::PostToFollowers {
                        post_id,
                        body: Some(fallback_text),
                        quote_url: None,
                        in_reply_to: None,
                    },
                )
                .await;
        }
    }

    // 二重防御: 元ポストが followers_only/direct（＝本来 create_repost で弾かれているはず）
    // なら Bsky コミットを行わない。呼び出し元の実装ミスに対する最終ガードとして再チェックする。
    let bsky_target =
        targets.bsky && meta.visibility != "followers_only" && meta.visibility != "direct";
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
                if let Err(e) = atp
                    .commit_repost(actor_id, &at_uri_clone, &at_cid_clone, now, Some(post_id))
                    .await
                {
                    tracing::error!("[create_note] ATP repost 失敗: {}", e);
                }
            });
        } else if origin != PostOrigin::BskyRemote && meta.ap_object_id.is_some() {
            // at_uri なし（Fedi リモートまたはローカル）→ Bsky フォールバック:
            // 本文はリポスト記号だけにし、元ポストURLを external embed で添付する。
            // リポストラッパー行（post_id）自体を PDS 上のテキストポストとしてコミットする。
            // commit_quote に post_id を渡すことで posts.at_uri/at_cid/at_rkey がこの行に
            // 書き込まれ、自前 Jetstream の自己エコー（save_bsky_post）が
            // `ON CONFLICT (at_uri) DO NOTHING` により重複ポストを作らなくなる
            // （このリポストと無関係な別ノートがタイムラインに現れなくなる）。
            let ap_id = meta.ap_object_id.clone().unwrap_or_default();
            let title = format!(
                "{} (@{}@{})",
                meta.display_name.as_deref().unwrap_or(&meta.username),
                meta.username,
                meta.domain
            );
            let description = meta.body.clone();
            let thumb_url = meta.first_image_url.clone().or(meta.avatar_url.clone());
            let state = state.clone();
            let atp = Arc::clone(&state.atp_service);
            tokio::spawn(async move {
                let thumb = prepare_external_thumb(&state, actor_id, thumb_url.as_deref()).await;
                let embed = BskyEmbed::External {
                    url: ap_id,
                    title,
                    description,
                    thumb,
                };
                if let Err(e) = atp
                    .commit_quote(actor_id, post_id, "🔁", vec![], Some(embed), now, None)
                    .await
                {
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
    ///
    /// `requested`はMisskey本家の語彙（`home`/`followers`/`specified`）も受け付け、
    /// 内部で対応するseiran語彙（`unlisted`/`followers_only`/`direct`）へ正規化してから
    /// 判定する（`handlers::misskey::convert::to_misskey_visibility`の逆変換に相当）。
    pub fn resolve_visibility(&self, requested: Option<&str>) -> Result<&'static str, ApiError> {
        let requested = requested.map(normalize_misskey_visibility);
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

/// Misskey本家の`visibility`語彙（`home`/`followers`/`specified`）をseiran語彙
/// （`unlisted`/`followers_only`/`direct`）へ正規化する。`public`およびseiran語彙が
/// そのまま来た場合・未知の値はそのまま通す（後続の判定で弾かれる）。
fn normalize_misskey_visibility(v: &str) -> &str {
    match v {
        "home" => "unlisted",
        "followers" => "followers_only",
        "specified" => "direct",
        other => other,
    }
}

/// リプライ先ポストの実体の有無から、返信の配信先制御
/// `(deliver_fedi_allowed, deliver_bsky_allowed)` を決定する。
///
/// ローカル投稿は `posts.deliver_fedi`/`deliver_bsky`（投稿作成時に実際に配送対象とした値）を
/// 直接見る。`ap_object_id` はローカル投稿なら `deliver_fedi` の値に関わらず常に生成される
/// （投稿URLとして常時pull取得可能なだけで、フォロワーへのpush配送有無とは別概念）ため、
/// その有無を配送可否のフラグとして使えない（`at_uri` は逆にコミットして初めて実体を持つため
/// 有無がそのまま配送可否と一致する、という非対称性がある）。
/// リモート投稿は逆に `deliver_fedi`/`deliver_bsky` カラムに意味が無い（受信経路はこのカラムに
/// 触れずDBデフォルト`true`のまま）ため、実体（`ap_object_id`/`at_uri`）の有無を直接見る。
///
/// classify_post の `is_local` 早期判定（`LocalOrSeiran` = 両実体持ち扱い）に頼ると、
/// ローカル投稿で片方のプロトコルにしか配送していない場合でも両方許可扱いになってしまい、
/// 親と無関係な独立ポストとして誤配信される（当初発見した不具合の修正）。
fn reply_delivery_allowed(meta: &PostDeliveryMeta) -> (bool, bool) {
    if meta.actor_type == "local" {
        (meta.deliver_fedi, meta.deliver_bsky)
    } else {
        (meta.ap_object_id.is_some(), meta.at_uri.is_some())
    }
}

/// リプライ先ポストの種別を判定し、配信先制御（元ポストが存在しないプロトコルには配信しない）と
/// ATP reply フィールドを組み立てる。`viewer_actor_id` はリプライしようとしている本人で、
/// リプライ先の投稿者とブロック関係にある場合はリプライ自体を拒否する（Bsky準拠のブロック定義）。
pub async fn resolve_reply_context(
    state: &AppState,
    reply_to_id_str: &str,
    viewer_actor_id: i64,
) -> Result<ReplyContext, ApiError> {
    let reply_to_id: i64 = reply_to_id_str
        .parse()
        .map_err(|_| ApiError::BadRequest("INVALID_REPLY_TO_ID".to_owned()))?;

    let meta = state
        .posts
        .find_delivery_meta(reply_to_id)
        .await
        .map_err(|e| ApiError::Internal(format!("reply 元ポスト取得失敗: {}", e)))?
        .ok_or(ApiError::NotFound("REPLY_TARGET_NOT_FOUND"))?;

    crate::handlers::target_resolve::check_not_blocked(state, viewer_actor_id, meta.actor_id)
        .await?;

    let (deliver_fedi_allowed, deliver_bsky_allowed) = reply_delivery_allowed(&meta);

    // ATP reply フィールド: Bsky 配信する場合かつ at_uri/at_cid が取得できる場合のみ設定
    let bsky_reply = if deliver_bsky_allowed {
        match (&meta.at_uri, &meta.at_cid) {
            (Some(uri), Some(cid)) => Some(BskyPostReply {
                root: BskyRefRecord {
                    cid: cid.clone(),
                    uri: uri.clone(),
                },
                parent: BskyRefRecord {
                    cid: cid.clone(),
                    uri: uri.clone(),
                },
            }),
            _ => None,
        }
    } else {
        None
    };

    let parent_local_actor_id = if meta.actor_type == "local" {
        Some(meta.actor_id)
    } else {
        None
    };

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

/// Fedi 配送で使う引用表現。
#[derive(Debug, PartialEq, Eq)]
pub enum ApQuote {
    /// AP 実体を持つ投稿は Misskey 互換の quoteUrl として配送する。
    Misskey(String),
    /// Bsky にしか実体がない投稿は、受信サーバーが AP オブジェクトとして解決できないため、
    /// bsky.app URL を本文末尾へ追記する。
    AppendUrl(String),
}

pub(crate) fn ap_delivery_quote_fields(
    text: &str,
    quote: Option<ApQuote>,
) -> (Option<String>, Option<String>) {
    match quote {
        Some(ApQuote::Misskey(url)) => (None, Some(url)),
        Some(ApQuote::AppendUrl(url)) => (Some(format!("{}\n\n{}", text, url)), None),
        None => (None, None),
    }
}

/// Bsky embed選択（#227）がURLで、かつ本文にそのURLが含まれない場合、ActivityPub配送用
/// 本文への追記が必要かどうかを判定する。Fedi（AP）にはBskyのembed概念が無く、本文でしか
/// URLを参照できないため、選択後に本文からそのURLを削除した「孤児」状態のままだとFedi側の
/// 読者だけがそのURLを一切見られなくなってしまう（マイケル指摘）。本文に既に含まれている
/// 場合、および引用投稿（`quote_embed_present`、`bsky_embed_choice`自体が無視される）は
/// 何もしない。
fn fedi_url_append_needed(
    text: &str,
    quote_embed_present: bool,
    choice: Option<&BskyEmbedChoice>,
) -> Option<String> {
    if quote_embed_present {
        return None;
    }
    match choice {
        Some(BskyEmbedChoice::Url { url }) if !text.contains(url.as_str()) => Some(url.clone()),
        _ => None,
    }
}

pub(crate) fn ap_quote_from_meta(meta: &PostDeliveryMeta) -> Option<ApQuote> {
    if meta.at_uri.is_some() && meta.ap_object_id.is_none() {
        meta.at_uri
            .as_deref()
            .map(at_uri_to_bsky_app_url)
            .map(ApQuote::AppendUrl)
    } else {
        meta.ap_object_id.clone().map(ApQuote::Misskey)
    }
}

/// 引用元ポストの種別から Bsky embed（引用埋め込み）と AP 向け引用表現を組み立てる。
pub async fn resolve_quote_embed(
    state: &AppState,
    actor_id: i64,
    quote_of_id: i64,
) -> (Option<BskyEmbed>, Option<ApQuote>) {
    let meta = match state.posts.find_delivery_meta(quote_of_id).await {
        Ok(Some(m)) => m,
        _ => return (None, None),
    };

    let origin = classify_post(
        meta.ap_object_id.as_deref(),
        meta.at_uri.as_deref(),
        meta.actor_type == "local",
    );

    let bsky_embed = if origin == PostOrigin::FediRemote {
        build_external_post_embed(state, actor_id, &meta).await
    } else if let (Some(uri), Some(cid)) = (&meta.at_uri, &meta.at_cid) {
        Some(BskyEmbed::Record {
            uri: uri.clone(),
            cid: cid.clone(),
        })
    } else {
        // AP/ATP の両IDを持つ投稿でも、AT CID が未取得ならネイティブ引用を構築できない。
        // このフォールバックでも空カードにせず、Fediリモートと同じメタデータを設定する。
        build_external_post_embed(state, actor_id, &meta).await
    };

    let ap_quote = ap_quote_from_meta(&meta);

    (bsky_embed, ap_quote)
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
    pub ap_quote: Option<ApQuote>,
    pub ap_in_reply_to: Option<String>,
    pub attachment_ids: Vec<i64>,
    /// Bsky embedの明示選択（#227、`resolve_bsky_embed`参照）。引用投稿（`bsky_quote_embed`が
    /// `Some`）の場合は無視される（引用embedと画像/動画/URL embedは共存しない）。
    pub bsky_embed_choice: Option<BskyEmbedChoice>,
    /// アンケート（#228、`posts.poll`と同じ形のJSON）。Bsky embed選択の`Poll`候補・
    /// AP `Question`配送の両方で使う。
    pub poll: Option<serde_json::Value>,
    /// CW（閲覧注意）ガイド文（#229）。`Some`の場合、Bsky配送は画像/動画/URL/アンケートの
    /// 候補選択を一切行わず（`bsky_embed_choice`・`bsky_quote_embed`も無視）、常に
    /// `build_cw_bsky_embed`のURLリンクカードのみを添付し、本文もこのガイド文に差し替える。
    /// AP配送では`summary`フィールドとして送る（本文・添付・アンケート・引用は通常通り）。
    pub content_warning: Option<String>,
}

/// CW（閲覧注意）投稿のBsky embedを組み立てる（#229）。投稿詳細ページのURLに
/// `#open_cw`（開いた状態を表すハッシュ）を付けたものを、「Open」という言語非依存の
/// タイトルでリンクカード化する（descriptionは無し、thumbも無し）。
fn build_cw_bsky_embed(local_domain: &str, post_id: i64) -> BskyEmbed {
    BskyEmbed::External {
        url: format!("https://{}/notes/{}#open_cw", local_domain, post_id),
        title: "Open".to_string(),
        description: String::new(),
        thumb: None,
    }
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
            state
                .enqueue_ap_delivery(
                    d.actor_id,
                    ApDeliveryKind::DirectMessage { post_id: d.post_id },
                )
                .await;
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

    // Bsky embed選択がURL（#227）の場合、ActivityPub配信でも同じURLを参照できるようにする
    // （マイケル指摘）。引用投稿（`bsky_quote_embed`がSome）は`bsky_embed_choice`自体が
    // 無視されるため対象外。CW（#229）中も`bsky_embed_choice`自体を無視するため対象外。
    let fedi_append_url: Option<String> = if bsky_target && d.content_warning.is_none() {
        fedi_url_append_needed(&d.text, d.bsky_quote_embed.is_some(), d.bsky_embed_choice.as_ref())
    } else {
        None
    };

    if bsky_target {
        // CW（#229）が設定されている場合、Bsky配送は画像/動画/URL/アンケート・引用embedの
        // 選択を一切行わず（隠された本文・添付物すべてを見るにはURLリンクカードから
        // seiranの記事詳細ページへ飛ぶ設計のため）、常にCWガイド文を本文として
        // build_cw_bsky_embedのリンクカード1件だけをコミットする。
        let bsky_source_text: &str = d.content_warning.as_deref().unwrap_or(&d.text);
        // メンション変換（変換失敗時は元テキストをそのまま使用する）
        // Bsky 配信用: `@username` → `@username.{local_domain}`、`@user@domain` → brid.gy ハンドル
        let (bsky_text, bsky_facets) = convert_mentions_for_bsky(
            bsky_source_text,
            &state.local_domain,
            &state.db,
            state.ap_client.http.as_ref(),
        )
        .await;

        if d.content_warning.is_some() {
            let embed = build_cw_bsky_embed(&state.local_domain, d.post_id);
            if let Err(e) = state
                .atp_service
                .commit_post(
                    d.actor_id,
                    d.post_id,
                    &bsky_text,
                    bsky_facets,
                    Some(embed),
                    d.now,
                    d.bsky_reply,
                )
                .await
            {
                tracing::error!("[create_note] ATP CW commit 失敗（投稿は保存済み）: {}", e);
            }
        } else if let Some(embed) = d.bsky_quote_embed {
            // 引用投稿: embed を付けて commit_quote を使う（画像/動画/URL embed選択と共存しない）
            if let Err(e) = state
                .atp_service
                .commit_quote(
                    d.actor_id,
                    d.post_id,
                    &bsky_text,
                    bsky_facets,
                    Some(embed),
                    d.now,
                    d.bsky_reply,
                )
                .await
            {
                tracing::error!(
                    "[create_note] ATP quote commit 失敗（投稿は保存済み）: {}",
                    e
                );
            }
        } else {
            // 選択（またはその省略時の固定優先順位）からBsky embedを解決する（#227）。
            // 選択された添付がBsky動画パイプライン結合未確定の場合のみ、ここで即座に
            // commit_postすると常にapp.bsky.embed.externalへフォールバックしてしまう
            // （一度externalでコミットされた投稿は再コミットされないため、以後video embed化
            // されることもない）。投稿ボタンを押すタイミングが早すぎるだけで起きる問題なので、
            // その添付1件についてだけBskyコミット自体をWorker（Job::BskyPostCommitDeferred）
            // に委譲し、結合完了を待ってからコミットする（2026-07-17 マイケル指摘・実機再現確認）。
            match resolve_bsky_embed(
                state,
                d.actor_id,
                d.post_id,
                &d.attachment_ids,
                &d.text,
                d.poll.as_ref(),
                d.bsky_embed_choice,
            )
            .await
            {
                BskyEmbedResolution::Pending(media_file_id) => {
                    let (reply_root, reply_parent) = split_bsky_reply(&d.bsky_reply);
                    state
                        .enqueue_bsky_post_commit_deferred(
                            d.actor_id,
                            d.post_id,
                            d.text.clone(),
                            media_file_id,
                            reply_root,
                            reply_parent,
                            d.now,
                        )
                        .await;
                }
                BskyEmbedResolution::Ready(embed) => {
                    if let Err(e) = state
                        .atp_service
                        .commit_post(
                            d.actor_id,
                            d.post_id,
                            &bsky_text,
                            bsky_facets,
                            embed,
                            d.now,
                            d.bsky_reply,
                        )
                        .await
                    {
                        tracing::error!("[create_note] ATP コミット失敗（投稿は保存済み）: {}", e);
                    }
                }
            }
        }
    }

    if d.targets.fedi {
        // body は渡さない。deliver_post_to_ap_followers 側で DB の投稿本文を取得し、
        // メンション解決（tag[]・<a> アンカー付与）まで一貫して行う（ただし上のURL追記が
        // 必要な場合、または引用URL追記が必要な場合はここで上書き本文を渡す）。
        let (quote_body, quote_url) = ap_delivery_quote_fields(&d.text, d.ap_quote);
        let body = match (quote_body, fedi_append_url) {
            (Some(b), Some(url)) => Some(format!("{}\n\n{}", b, url)),
            (Some(b), None) => Some(b),
            (None, Some(url)) => Some(format!("{}\n\n{}", d.text, url)),
            (None, None) => None,
        };
        state
            .enqueue_ap_delivery(
                d.actor_id,
                ApDeliveryKind::PostToFollowers {
                    post_id: d.post_id,
                    body,
                    quote_url,
                    in_reply_to: d.ap_in_reply_to,
                },
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ap_delivery_quote_fields, ap_quote_from_meta, at_uri_to_bsky_app_url, build_cw_bsky_embed,
        classify_post, fedi_url_append_needed, resolve_poll_embed, ApQuote, BskyEmbed,
        BskyEmbedChoice, PostOrigin, ReplyContext,
    };
    use crate::error::ApiError;
    use seiran_common::repository::post::PostDeliveryMeta;

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

    fn delivery_meta(ap_object_id: Option<&str>, at_uri: Option<&str>) -> PostDeliveryMeta {
        PostDeliveryMeta {
            actor_id: 1,
            ap_object_id: ap_object_id.map(str::to_owned),
            at_uri: at_uri.map(str::to_owned),
            at_cid: None,
            domain: "example.com".to_owned(),
            actor_type: "fedi".to_owned(),
            display_name: None,
            username: "alice".to_owned(),
            body: "quoted post".to_owned(),
            avatar_url: None,
            first_image_url: None,
            visibility: "public".to_owned(),
            thread_root_post_id: None,
            // リモート想定のヘルパーのため意味を持たない（DBデフォルトのtrue固定を模す）。
            deliver_fedi: true,
            deliver_bsky: true,
        }
    }

    /// ローカル投稿想定の `PostDeliveryMeta`。`ap_object_id` はローカルなら
    /// `deliver_fedi` の値に関わらず常に生成されるため常に `Some` を渡す。
    fn local_delivery_meta(deliver_fedi: bool, deliver_bsky: bool) -> PostDeliveryMeta {
        PostDeliveryMeta {
            actor_type: "local".to_owned(),
            at_uri: deliver_bsky.then(|| "at://did/x/y".to_owned()),
            deliver_fedi,
            deliver_bsky,
            ..delivery_meta(Some("https://local.example/notes/1"), None)
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
        assert_eq!(
            ctx.resolve_visibility(Some("followers_only")).unwrap(),
            "direct"
        );
    }

    #[test]
    fn resolve_visibility_followers_only_parent_forces_followers_only() {
        let ctx = ctx_with_parent_visibility(Some("followers_only"));
        assert_eq!(ctx.resolve_visibility(None).unwrap(), "followers_only");
        assert_eq!(
            ctx.resolve_visibility(Some("public")).unwrap(),
            "followers_only"
        );
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
        assert_eq!(
            ctx.resolve_visibility(Some("followers_only")).unwrap(),
            "followers_only"
        );
        assert_eq!(
            ctx.resolve_visibility(Some("unlisted")).unwrap(),
            "unlisted"
        );
    }

    #[test]
    fn resolve_visibility_unlisted_parent_rejects_direct() {
        let ctx = ctx_with_parent_visibility(Some("unlisted"));
        assert!(matches!(
            ctx.resolve_visibility(Some("direct")),
            Err(ApiError::BadRequest(_))
        ));
    }

    #[test]
    fn resolve_visibility_public_parent_allows_any_valid_value() {
        let ctx = ctx_with_parent_visibility(Some("public"));
        assert_eq!(
            ctx.resolve_visibility(Some("unlisted")).unwrap(),
            "unlisted"
        );
        assert_eq!(
            ctx.resolve_visibility(Some("followers_only")).unwrap(),
            "followers_only"
        );
        assert_eq!(ctx.resolve_visibility(Some("direct")).unwrap(), "direct");
    }

    #[test]
    fn resolve_visibility_rejects_unknown_value() {
        let ctx = ctx_with_parent_visibility(None);
        assert!(matches!(
            ctx.resolve_visibility(Some("bogus")),
            Err(ApiError::BadRequest(_))
        ));
    }

    #[test]
    fn resolve_visibility_accepts_misskey_vocabulary() {
        // Misskey本家語彙（home/followers/specified）をAria等が送ってきても、
        // seiran語彙（unlisted/followers_only/direct）と同じ結果になること。
        let ctx = ctx_with_parent_visibility(None);
        assert_eq!(ctx.resolve_visibility(Some("home")).unwrap(), "unlisted");
        assert_eq!(
            ctx.resolve_visibility(Some("followers")).unwrap(),
            "followers_only"
        );
        assert_eq!(ctx.resolve_visibility(Some("specified")).unwrap(), "direct");
    }

    #[test]
    fn resolve_visibility_misskey_vocabulary_respects_parent_constraints() {
        let ctx = ctx_with_parent_visibility(Some("direct"));
        // DMスレッド内の返信でMisskey語彙を送っても、direct強制から離脱できない。
        assert_eq!(ctx.resolve_visibility(Some("home")).unwrap(), "direct");

        let ctx = ctx_with_parent_visibility(Some("unlisted"));
        assert!(matches!(
            ctx.resolve_visibility(Some("specified")),
            Err(ApiError::BadRequest(_))
        ));
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
        assert_eq!(
            at_uri_to_bsky_app_url("at://did:plc:abc123"),
            "at://did:plc:abc123"
        );
    }

    #[test]
    fn ap_quote_uses_misskey_fields_for_ap_object() {
        assert_eq!(
            ap_delivery_quote_fields(
                "comment",
                Some(ApQuote::Misskey("https://fedi.example/notes/1".to_owned()))
            ),
            (None, Some("https://fedi.example/notes/1".to_owned()))
        );
    }

    #[test]
    fn ap_quote_appends_bsky_url_to_body() {
        assert_eq!(
            ap_delivery_quote_fields(
                "comment",
                Some(ApQuote::AppendUrl(
                    "https://bsky.app/profile/did:plc:test/post/abc".to_owned()
                ))
            ),
            (
                Some("comment\n\nhttps://bsky.app/profile/did:plc:test/post/abc".to_owned()),
                None
            )
        );
    }

    #[test]
    fn fedi_url_append_needed_appends_when_url_missing_from_text() {
        let choice = BskyEmbedChoice::Url {
            url: "https://example.com/article".to_owned(),
        };
        assert_eq!(
            fedi_url_append_needed("本文からURLを消した後", false, Some(&choice)),
            Some("https://example.com/article".to_owned())
        );
    }

    #[test]
    fn fedi_url_append_needed_no_op_when_url_already_in_text() {
        let choice = BskyEmbedChoice::Url {
            url: "https://example.com/article".to_owned(),
        };
        assert_eq!(
            fedi_url_append_needed(
                "見て https://example.com/article",
                false,
                Some(&choice)
            ),
            None
        );
    }

    #[test]
    fn fedi_url_append_needed_no_op_for_non_url_choice() {
        let choice = BskyEmbedChoice::Images;
        assert_eq!(fedi_url_append_needed("本文", false, Some(&choice)), None);
        assert_eq!(fedi_url_append_needed("本文", false, None), None);
    }

    #[test]
    fn fedi_url_append_needed_no_op_when_quote_embed_present() {
        let choice = BskyEmbedChoice::Url {
            url: "https://example.com/article".to_owned(),
        };
        assert_eq!(fedi_url_append_needed("本文", true, Some(&choice)), None);
    }

    #[test]
    fn build_cw_bsky_embed_uses_open_cw_hash_and_open_title() {
        let embed = build_cw_bsky_embed("seiran.example", 42);
        match embed {
            BskyEmbed::External {
                url,
                title,
                description,
                thumb,
            } => {
                assert_eq!(url, "https://seiran.example/notes/42#open_cw");
                assert_eq!(title, "Open");
                assert_eq!(description, "");
                assert!(thumb.is_none());
            }
            _ => panic!("expected External embed"),
        }
    }

    #[test]
    fn resolve_poll_embed_builds_bullet_list_description_and_self_url() {
        let poll = serde_json::json!({
            "multiple": false,
            "options": [
                {"name": "選択肢A", "votes": 0},
                {"name": "選択肢B", "votes": 0}
            ]
        });
        let embed = resolve_poll_embed("seiran.example", 42, &poll);
        match embed {
            BskyEmbed::External {
                url,
                title,
                description,
                thumb,
            } => {
                assert_eq!(url, "https://seiran.example/notes/42");
                assert_eq!(title, "");
                assert_eq!(description, "- 選択肢A\n- 選択肢B");
                assert!(thumb.is_none());
            }
            _ => panic!("expected External embed"),
        }
    }

    #[test]
    fn ap_quote_from_meta_uses_misskey_fields_for_ap_object() {
        let meta = delivery_meta(
            Some("https://fedi.example/notes/1"),
            Some("at://did:plc:test/app.bsky.feed.post/abc"),
        );

        assert_eq!(
            ap_quote_from_meta(&meta),
            Some(ApQuote::Misskey("https://fedi.example/notes/1".to_owned()))
        );
    }

    #[test]
    fn ap_quote_from_meta_appends_bsky_only_url() {
        let meta = delivery_meta(None, Some("at://did:plc:test/app.bsky.feed.post/abc"));

        assert_eq!(
            ap_quote_from_meta(&meta),
            Some(ApQuote::AppendUrl(
                "https://bsky.app/profile/did:plc:test/post/abc".to_owned()
            ))
        );
    }

    #[test]
    fn classify_post_local_domain_match() {
        // is_local=true の場合は ap_object_id / at_uri の値によらずローカル扱い
        assert_eq!(classify_post(None, None, true), PostOrigin::LocalOrSeiran);
    }

    #[test]
    fn classify_post_seiran_remote_has_both_ids() {
        assert_eq!(
            classify_post(Some("https://a/notes/1"), Some("at://did/x/y"), false),
            PostOrigin::LocalOrSeiran
        );
    }

    #[test]
    fn classify_post_fedi_remote_ap_only() {
        assert_eq!(
            classify_post(Some("https://mastodon.example/notes/1"), None, false),
            PostOrigin::FediRemote
        );
    }

    #[test]
    fn classify_post_bsky_remote_at_uri_only() {
        assert_eq!(
            classify_post(None, Some("at://did/x/y"), false),
            PostOrigin::BskyRemote
        );
    }

    #[test]
    fn classify_post_unknown_defaults_to_local() {
        assert_eq!(classify_post(None, None, false), PostOrigin::LocalOrSeiran);
    }

    // ─── reply_delivery_allowed ────────────────────────────────────────────
    // リプライの配信先制御（間違えると親と無関係な独立ポストとして誤配信され、
    // スレッドが繋がらない不具合になるため、実体の有無の組み合わせを網羅する）。
    // リモート投稿は実体（ap_object_id/at_uri）の有無、ローカル投稿は
    // deliver_fedi/deliver_bsky カラムの値で判定が分かれる点に注意
    // （ローカル投稿の ap_object_id は deliver_fedi に関わらず常に存在するため）。

    #[test]
    fn reply_delivery_allowed_remote_both_entities_allows_both() {
        let meta = delivery_meta(Some("https://a/notes/1"), Some("at://did/x/y"));
        assert_eq!(super::reply_delivery_allowed(&meta), (true, true));
    }

    #[test]
    fn reply_delivery_allowed_remote_fedi_entity_only_disallows_bsky() {
        // Fediリモート投稿（ap_object_idはあるがat_uriが無い）への返信は、
        // Bsky上に親を持たないため Bsky 配信してはならない。
        let meta = delivery_meta(Some("https://a/notes/1"), None);
        assert_eq!(super::reply_delivery_allowed(&meta), (true, false));
    }

    #[test]
    fn reply_delivery_allowed_remote_bsky_entity_only_disallows_fedi() {
        let meta = delivery_meta(None, Some("at://did/x/y"));
        assert_eq!(super::reply_delivery_allowed(&meta), (false, true));
    }

    #[test]
    fn reply_delivery_allowed_remote_no_entity_disallows_both() {
        let meta = delivery_meta(None, None);
        assert_eq!(super::reply_delivery_allowed(&meta), (false, false));
    }

    #[test]
    fn reply_delivery_allowed_local_both_delivered_allows_both() {
        let meta = local_delivery_meta(true, true);
        assert_eq!(super::reply_delivery_allowed(&meta), (true, true));
    }

    #[test]
    fn reply_delivery_allowed_local_fedi_only_disallows_bsky() {
        // fedi配送のみのローカル投稿（deliver_bsky=false）への返信は、ap_object_idが
        // 常に存在していても Bsky 実体を持たないため Bsky 配信してはならない
        // （実体の有無だけで判定すると誤って許可されてしまう、当初発見した不具合）。
        let meta = local_delivery_meta(true, false);
        assert_eq!(super::reply_delivery_allowed(&meta), (true, false));
    }

    #[test]
    fn reply_delivery_allowed_local_bsky_only_disallows_fedi() {
        // Bsky配送のみのローカル投稿（deliver_fedi=false）への返信は、ap_object_idが
        // 常に生成されていても Fedi 実体の有無では判定できないため deliver_fedi を直接見る。
        let meta = local_delivery_meta(false, true);
        assert_eq!(super::reply_delivery_allowed(&meta), (false, true));
    }
}

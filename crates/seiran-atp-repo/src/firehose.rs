//! Jetstream（Bluesky公式の軽量フィルタ済みFirehose）WebSocket クライアント
//!
//! `wss://jetstream1.us-east.bsky.network/subscribe` に `wantedCollections` を指定して
//! 接続し、`app.bsky.feed.post`（新規投稿）と `app.bsky.feed.like`（リアクション連携）の
//! create/delete のみを受信する。Jetstream は Relay Firehose を購読して dag-cbor から
//! 既にJSONへデコード済みのレコードを配信するため、CBOR/CAR/CIDの自前デコードは不要。
//!
//! 投稿はイベントに同梱されるレコード本体（text/createdAt）をそのまま保存する
//! （Jetstream はほぼリアルタイムなので、旧実装にあった AppView 再取得＋インデックス
//! 遅延リトライは不要）。`record.reply.parent.uri` が付いている場合は、その親投稿が
//! `posts.at_uri` として既知（＝こちらの投稿への返信）かを調べ、既知なら
//! `posts.reply_to_post_id` を設定してリプライとして保存する。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use sqlx::{PgPool, Row};
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use seiran_common::atp::{
    ParsedAttachment, ParsedFacet, ParsedLinkCard, apply_bsky_facets, fetch_bsky_profile,
    parse_bsky_embed_attachments, parse_bsky_embed_link_card, parse_bsky_embed_quote_uri,
};
use seiran_common::jetstream_control::fetch_wanted_dids_touch;
use seiran_common::jetstream_leader::{self, JetstreamLeaderElector};
use seiran_common::queue::worker::priority;
use seiran_common::repository::{
    ActorRepository, EmojiRepository, HashtagRepository, NotificationKind, NotificationRepository,
    PgActorRepository, PgEmojiRepository, PgFollowRepository, PgHashtagRepository,
    PgNotificationRepository, PgPostRepository, PgReactionRepository, PostRepository,
    ReactionRepository, extract_shortcode_candidates, parse_reaction_shortcode_and_host,
};
use seiran_common::streaming::{ChannelScope, broadcast_reaction_update};
use seiran_common::traits::{Job, JobQueue};
use seiran_common::{StreamHub, generate_snowflake_id};

const JETSTREAM_BASE_URL: &str = "wss://jetstream1.us-east.bsky.network/subscribe?wantedCollections=app.bsky.feed.post&wantedCollections=app.bsky.feed.like&wantedCollections=app.bsky.feed.repost";

/// `wantedDids` 絞り込みリスト（フォロイー + リストメンバーの Bsky DID 集合）を
/// 再構築すべきか、受信ループ内で定期ポーリングする間隔。フォロー変更等は
/// リアルタイム性が必須ではなく、cursorによりこの間の取りこぼしも無いため、
/// 短すぎない間隔で十分（DBポーリング負荷を抑える）。
const WANTED_DIDS_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// サーバー停止中に発生したイベントを取りこぼさないよう、直近処理した Jetstream
/// イベントの `time_us`（マイクロ秒 Unix タイムスタンプ）を `site_settings`
/// （汎用KVテーブル。Doc1 §1.11）に永続化し、再接続時に `cursor` パラメータとして
/// 引き継ぐ。書き込み頻度を抑えるため、受信ループ内で一定間隔ごとにのみ保存する。
const JETSTREAM_CURSOR_KEY: &str = "jetstream_cursor";
const JETSTREAM_CURSOR_SAVE_INTERVAL: Duration = Duration::from_secs(5);

/// Jetstream接続の起動・停止を、Redisによるリーダー選出（`seiran_common::jetstream_leader`）
/// の結果に応じて切り替える。`docker-compose.mono.yml`の`--scale seiran-server=N`（無停止
/// バージョンアップ中の一時的な複数起動）や`firehose`ロールの複数インスタンス起動時に、
/// Jetstream WebSocket接続が重複して張られるのを防ぐ（Doc6既知の課題）。
///
/// `redis_url`が無い場合、またはRedisとの通信に失敗し続ける場合は、ロールに応じて
/// フェイルオープン/フェイルクローズする（`is_monolith`）。monolith（`all`ロール）は
/// 複数起動時の非効率を許容する方針のため接続を維持し、split-role構成の`firehose`ロールは
/// Redisが死ねばジョブキュー等の他機能も共倒れになるため接続を切る。
pub async fn run(
    pool: PgPool,
    http: Arc<reqwest::Client>,
    stream_hub: Arc<StreamHub>,
    redis_url: Option<String>,
    is_monolith: bool,
    job_queue: Arc<dyn JobQueue>,
) {
    let mut elector: Option<JetstreamLeaderElector> = None;
    let mut current_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut poll = tokio::time::interval(jetstream_leader::LEASE_CHECK_INTERVAL);

    loop {
        poll.tick().await;

        let should_run = match &redis_url {
            None => is_monolith,
            Some(url) => {
                if elector.is_none() {
                    match JetstreamLeaderElector::connect(url, jetstream_leader::DEFAULT_LEADER_KEY)
                        .await
                    {
                        Ok(e) => elector = Some(e),
                        Err(e) => tracing::error!("[Jetstream] Redis接続失敗: {}", e),
                    }
                }
                match &elector {
                    Some(e) => match e.try_acquire_or_renew().await {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::error!(
                                "[Jetstream] Redisリース確認失敗: {}。再接続を試みます。",
                                e
                            );
                            elector = None;
                            is_monolith
                        }
                    },
                    None => is_monolith,
                }
            }
        };

        match (should_run, current_task.is_some()) {
            (true, false) => {
                tracing::info!(
                    "[Jetstream] リーダーに昇格（またはRedis未使用の単独運用）。接続開始。"
                );
                let pool = pool.clone();
                let http = Arc::clone(&http);
                let hub = Arc::clone(&stream_hub);
                let queue = Arc::clone(&job_queue);
                current_task = Some(tokio::spawn(run_jetstream_loop(pool, http, hub, queue)));
            }
            (false, true) => {
                tracing::info!("[Jetstream] リーダーでなくなったため切断。");
                if let Some(task) = current_task.take() {
                    task.abort();
                }
            }
            _ => {}
        }
    }
}

/// Jetstream接続を維持し続けるループ（エラー時は指数バックオフで再接続）。
/// リーダー選出で「非リーダー」と判定されると、呼び出し元がこのタスクごと`abort`する。
async fn run_jetstream_loop(
    pool: PgPool,
    http: Arc<reqwest::Client>,
    stream_hub: Arc<StreamHub>,
    job_queue: Arc<dyn JobQueue>,
) {
    let mut backoff_secs = 2u64;

    loop {
        match connect_and_process(&pool, &http, &stream_hub, &job_queue).await {
            Ok(()) => {
                tracing::info!("[Jetstream] 接続終了（正常）。再接続します。");
                backoff_secs = 2;
            }
            Err(e) => {
                tracing::error!(
                    "[Jetstream] エラー: {}。{}秒後に再接続します。",
                    e,
                    backoff_secs
                );
                sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(120);
            }
        }
    }
}

async fn load_jetstream_cursor(pool: &PgPool) -> Option<i64> {
    sqlx::query("SELECT value FROM site_settings WHERE key = $1")
        .bind(JETSTREAM_CURSOR_KEY)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .and_then(|row| row.try_get::<String, _>("value").ok())
        .and_then(|v| v.parse::<i64>().ok())
}

async fn save_jetstream_cursor(pool: &PgPool, time_us: i64) {
    if let Err(e) = sqlx::query(
        "INSERT INTO site_settings (key, value, updated_at) VALUES ($1, $2, NOW())
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()",
    )
    .bind(JETSTREAM_CURSOR_KEY)
    .bind(time_us.to_string())
    .execute(pool)
    .await
    {
        tracing::error!("[Jetstream] cursor保存失敗: {}", e);
    }
}

/// cursorの`time_us`だけを取り出すための最小限のパース対象（`identity`/`account`
/// イベントも含め、全メッセージ種別に付与される）。
#[derive(Deserialize)]
struct JetstreamTimeUs {
    time_us: i64,
}

/// ローカルユーザーのフォロー先、またはいずれかのリストのメンバーである Bsky
/// アクターの DID 集合を取得する。`wantedDids` としてJetstreamへ渡し、サーバー側で
/// 無関係な投稿を除外してもらう。退会済み（`withdrawn_at`設定済み）ローカル
/// ユーザーのフォロー・所有リストは対象から除外する。
async fn load_wanted_dids(pool: &PgPool) -> Vec<String> {
    // `follows`/`list_members`（少数行）を起点にJOINでDIDを引く。`actors`側（at_didを
    // 持つ全件、Bsky経由でupsertされた既知アクター全体）から出発してEXISTSで判定すると
    // フルスキャンになり本末転倒（実測、既知アクター数十万件規模で1秒近くかかった）。
    let rows = sqlx::query(
        "SELECT DISTINCT a.at_did AS did
         FROM actors a
         JOIN follows f ON f.target_actor_id = a.id
         JOIN actors follower ON follower.id = f.follower_actor_id
         WHERE a.at_did IS NOT NULL AND f.status = 'accepted'
           AND follower.actor_type = 'local' AND follower.withdrawn_at IS NULL
         UNION
         SELECT DISTINCT a.at_did AS did
         FROM actors a
         JOIN list_members lm ON lm.actor_id = a.id
         JOIN lists l ON l.id = lm.list_id
         JOIN actors owner ON owner.id = l.owner_actor_id
         WHERE a.at_did IS NOT NULL AND owner.withdrawn_at IS NULL",
    )
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => rows
            .iter()
            .filter_map(|r| r.try_get::<String, _>("did").ok())
            .collect(),
        Err(e) => {
            tracing::error!(
                "[Jetstream] wantedDids取得失敗（無絞り込みで接続します）: {}",
                e
            );
            Vec::new()
        }
    }
}

/// `JETSTREAM_BASE_URL` に `wantedDids` を付与した接続URLを組み立てる。
/// 対象DIDが1件も無ければ絞り込みなし（全世界のpost/like）で接続する
/// （初回起動直後で誰もフォローしていない等のレアケース向けフォールバック）。
fn build_jetstream_url(cursor: Option<i64>, wanted_dids: &[String]) -> String {
    let mut url = JETSTREAM_BASE_URL.to_string();
    for did in wanted_dids {
        url.push_str("&wantedDids=");
        url.push_str(did);
    }
    if let Some(c) = cursor {
        url.push_str(&format!("&cursor={}", c));
    }
    url
}

async fn connect_and_process(
    pool: &PgPool,
    http: &Arc<reqwest::Client>,
    stream_hub: &Arc<StreamHub>,
    job_queue: &Arc<dyn JobQueue>,
) -> Result<(), String> {
    let cursor = load_jetstream_cursor(pool).await;
    let wanted_dids = load_wanted_dids(pool).await;
    let wanted_dids_touch_at_connect = fetch_wanted_dids_touch(pool).await;
    let url = build_jetstream_url(cursor, &wanted_dids);
    tracing::info!(
        "[Jetstream] 接続中（wantedDids {}件）: {}",
        wanted_dids.len(),
        url
    );

    let (mut ws_stream, _) = connect_async(&url)
        .await
        .map_err(|e| format!("WebSocket 接続失敗: {}", e))?;

    tracing::info!("[Jetstream] 接続成功。イベント受信中...");

    let mut last_saved_at = tokio::time::Instant::now() - JETSTREAM_CURSOR_SAVE_INTERVAL;
    let mut wanted_dids_poll = tokio::time::interval(WANTED_DIDS_POLL_INTERVAL);
    wanted_dids_poll.tick().await; // 初回tickは即座に発火するので消費しておく

    loop {
        tokio::select! {
            msg = ws_stream.next() => {
                let Some(msg) = msg else { break; };
                let msg = msg.map_err(|e| format!("WebSocket 受信エラー: {}", e))?;

                if let Message::Text(text) = msg {
                    if let Ok(t) = serde_json::from_str::<JetstreamTimeUs>(&text)
                        && last_saved_at.elapsed() >= JETSTREAM_CURSOR_SAVE_INTERVAL
                    {
                        save_jetstream_cursor(pool, t.time_us).await;
                        last_saved_at = tokio::time::Instant::now();
                    }

                    if let Err(e) = process_message(&text, pool, http, stream_hub, job_queue).await {
                        tracing::error!("[Jetstream] メッセージ処理エラー（スキップ）: {}", e);
                    }
                }
            }
            _ = wanted_dids_poll.tick() => {
                let current_touch = fetch_wanted_dids_touch(pool).await;
                if current_touch != wanted_dids_touch_at_connect {
                    tracing::info!("[Jetstream] wantedDids変更を検知。再接続します。");
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

/// Jetstream の commit イベント（`kind: "commit"`）。`identity`/`account` は無視する。
#[derive(Deserialize)]
struct JetstreamEvent {
    did: String,
    kind: String,
    commit: Option<JetstreamCommit>,
}

#[derive(Deserialize)]
struct JetstreamCommit {
    operation: String, // "create" | "update" | "delete"
    collection: String,
    rkey: String,
    /// create/update のみ存在。デコード済みのレコード本体（レコードの生 JSON）。
    #[serde(default)]
    record: Option<JsonValue>,
    /// create/update のみ存在。
    #[serde(default)]
    cid: Option<String>,
}

/// Bsky投稿本文中のカスタム絵文字（`:shortcode:`）を、このサーバーの `custom_emojis` と
/// 照合して emoji_map を構築する（#126）。ネイティブ投稿作成（`handlers/notes/mod.rs`の
/// `create_regular_post`）と同じ解決ロジック。ATP Jetstream経由の投稿保存にはこの解決が
/// 無く、常に空のemoji_mapで保存されていたため、`:shortcode:` が画像化されない不具合があった。
/// 解決に失敗しても投稿自体は継続する（絵文字がテキストのまま出るだけ）。
async fn resolve_local_emoji_map(pool: &PgPool, text: &str) -> JsonValue {
    let shortcode_candidates = extract_shortcode_candidates(text);
    if shortcode_candidates.is_empty() {
        return JsonValue::Object(Default::default());
    }
    let emoji_repo = PgEmojiRepository::new(pool.clone());
    let pairs = match emoji_repo
        .find_urls_by_shortcodes(&shortcode_candidates)
        .await
    {
        Ok(pairs) => pairs,
        Err(e) => {
            tracing::error!("[Jetstream] 絵文字ショートコード解決失敗: {}", e);
            Vec::new()
        }
    };
    JsonValue::Object(
        pairs
            .into_iter()
            .map(|(code, url)| (format!(":{}:", code), JsonValue::String(url)))
            .collect(),
    )
}

async fn process_message(
    text: &str,
    pool: &PgPool,
    http: &Arc<reqwest::Client>,
    stream_hub: &Arc<StreamHub>,
    job_queue: &Arc<dyn JobQueue>,
) -> Result<(), String> {
    let event: JetstreamEvent =
        serde_json::from_str(text).map_err(|e| format!("JSON パースエラー: {}", e))?;

    if event.kind != "commit" {
        return Ok(());
    }
    let Some(commit) = event.commit else {
        return Ok(());
    };
    let did = event.did;

    match commit.collection.as_str() {
        "app.bsky.feed.post" => {
            if commit.operation == "delete" {
                let at_uri = format!("at://{}/app.bsky.feed.post/{}", did, commit.rkey);
                let pool2 = pool.clone();
                tokio::spawn(async move {
                    handle_inbound_post_delete(&pool2, &at_uri).await;
                });
                return Ok(());
            }
            if commit.operation != "create" {
                return Ok(());
            }
            let (Some(record), Some(cid)) = (commit.record, commit.cid) else {
                return Ok(());
            };
            let Some(body_text) = record.get("text").and_then(|v| v.as_str()) else {
                return Ok(());
            };
            // リンク・メンションの facet（byteStart/byteEnd で示される範囲）。
            // 未指定・パース失敗時は空のまま（投稿保存自体はブロックしない）。
            let parsed_facets: Vec<ParsedFacet> = record
                .get("facets")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let Some(created_at) = record
                .get("createdAt")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
            else {
                return Ok(());
            };
            // リプライなら reply.parent.uri を見て、親がこちらの既知投稿（at_uri 保存済み）か
            // どうかで reply_to_post_id を解決する（親が不明なら通常投稿として扱う）。
            let reply_parent_uri = record
                .get("reply")
                .and_then(|r| r.get("parent"))
                .and_then(|p| p.get("uri"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // 添付（画像・動画）。CDN URL は DID + blob CID から決定的に組み立てる。
            let attachments: Vec<ParsedAttachment> = record
                .get("embed")
                .map(|embed| parse_bsky_embed_attachments(embed, &did))
                .unwrap_or_default();

            // 引用先の at:// URI（#116）。`app.bsky.embed.record`/`recordWithMedia` のみ対象。
            let quote_uri: Option<String> =
                record.get("embed").and_then(parse_bsky_embed_quote_uri);

            // URLカード（YouTube/Spotify/x.com/一般）。GIFピッカー由来の`external`は
            // 既に`attachments`側で動画として扱われているためここには含まれない。
            let link_card: Option<ParsedLinkCard> = record
                .get("embed")
                .and_then(|embed| parse_bsky_embed_link_card(embed, &did));

            // この DID のアクターが「ローカルユーザーにフォローされている」、または
            // 「いずれかのリストに含まれている」場合のみ保存対象とする（リスト機能 #63:
            // 誰にもフォローされていないBskyユーザーでも、リストに入れれば投稿を受信できる）。
            // 単に actors テーブルに存在するだけでは不十分（いいね等をきっかけに resolve_or_upsert_bsky_actor
            // で無関係なアクターが actors へ upsert され、その投稿まで際限なく取り込まれてしまうため。
            // 2026-07: 実際にこの経路で posts が104万行超まで膨張する不具合があった）。
            let actor_row = sqlx::query(
                "SELECT a.id, a.username, a.display_name, a.avatar_url
                 FROM actors a
                 WHERE a.at_did = $1
                   AND (
                     EXISTS (
                       SELECT 1 FROM follows f
                       JOIN actors follower ON follower.id = f.follower_actor_id
                       WHERE f.target_actor_id = a.id AND f.status = 'accepted' AND follower.actor_type = 'local'
                     )
                     OR EXISTS (SELECT 1 FROM list_members lm WHERE lm.actor_id = a.id)
                   )
                 LIMIT 1",
            )
            .bind(&did)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("DB検索失敗: {}", e))?;

            let Some(actor_row) = actor_row else {
                return Ok(());
            };
            let actor_id: i64 = actor_row.try_get("id").unwrap_or(0);
            let username: String = actor_row.try_get("username").unwrap_or_default();
            let display_name: Option<String> = actor_row.try_get("display_name").unwrap_or(None);
            let avatar_url: Option<String> = actor_row.try_get("avatar_url").unwrap_or(None);

            let at_uri = format!("at://{}/app.bsky.feed.post/{}", did, commit.rkey);

            // 重複チェック
            let already_saved = sqlx::query("SELECT id FROM posts WHERE at_uri = $1 LIMIT 1")
                .bind(&at_uri)
                .fetch_optional(pool)
                .await
                .map_err(|e| format!("重複チェック失敗: {}", e))?
                .is_some();

            if already_saved {
                return Ok(());
            }

            tracing::info!("[Jetstream] 新規ポスト検出: {}", at_uri);

            let pool2 = pool.clone();
            let hub2 = Arc::clone(stream_hub);
            let queue2 = Arc::clone(job_queue);
            let http2 = Arc::clone(http);
            let at_uri2 = at_uri.clone();
            let author_did = did.clone();
            let body_text = body_text.to_string();

            tokio::spawn(async move {
                let posts_repo = PgPostRepository::new(pool2.clone());
                let reply_to_post_id = match &reply_parent_uri {
                    Some(parent_uri) => {
                        match posts_repo.find_id_and_actor_by_at_uri(parent_uri).await {
                            Ok(Some((parent_post_id, _))) => Some(parent_post_id),
                            Ok(None) => None,
                            Err(e) => {
                                tracing::error!(
                                    "[Jetstream] リプライ親投稿検索失敗（通常投稿として保存）: {}",
                                    e
                                );
                                None
                            }
                        }
                    }
                    None => None,
                };
                // 引用先のローカル post_id 解決（#116）。引用先が未取得のリモート投稿等で
                // ローカルDBに存在しない場合は通常投稿として保存する（quote_of_post_id=None）。
                let quote_of_post_id = match &quote_uri {
                    Some(uri) => match posts_repo.find_id_and_actor_by_at_uri(uri).await {
                        Ok(Some((quote_post_id, _))) => Some(quote_post_id),
                        Ok(None) => None,
                        Err(e) => {
                            tracing::error!(
                                "[Jetstream] 引用元投稿検索失敗（通常投稿として保存）: {}",
                                e
                            );
                            None
                        }
                    },
                    None => None,
                };
                let (body_text, mention_facets) = apply_bsky_facets(&body_text, parsed_facets);
                let emoji_map = resolve_local_emoji_map(&pool2, &body_text).await;
                save_bsky_post(
                    &pool2,
                    &queue2,
                    &http2,
                    &hub2,
                    &at_uri2,
                    &author_did,
                    &cid,
                    &body_text,
                    &mention_facets,
                    &emoji_map,
                    created_at,
                    actor_id,
                    &username,
                    display_name.as_deref(),
                    avatar_url.as_deref(),
                    reply_to_post_id,
                    quote_of_post_id,
                    attachments,
                    link_card,
                )
                .await;
            });
        }

        "app.bsky.feed.repost" => {
            if commit.operation == "delete" {
                // `at_uri`ベースの論理削除は投稿・リポストで共通（`handle_inbound_post_delete`、
                // `soft_delete_by_at_uri`は対象コレクションを問わない）。
                let at_uri = format!("at://{}/app.bsky.feed.repost/{}", did, commit.rkey);
                let pool2 = pool.clone();
                tokio::spawn(async move {
                    handle_inbound_post_delete(&pool2, &at_uri).await;
                });
                return Ok(());
            }
            if commit.operation != "create" {
                return Ok(());
            }
            let Some(record) = commit.record else {
                return Ok(());
            };
            let Some(subject_uri) = record
                .get("subject")
                .and_then(|subject| subject.get("uri"))
                .and_then(|value| value.as_str())
            else {
                return Ok(());
            };
            let at_uri = format!("at://{}/app.bsky.feed.repost/{}", did, commit.rkey);
            let created_at = record
                .get("createdAt")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok())
                .unwrap_or_else(chrono::Utc::now);
            let pool2 = pool.clone();
            let http2 = Arc::clone(http);
            let hub2 = Arc::clone(stream_hub);
            let queue2 = Arc::clone(job_queue);
            let subject_uri = subject_uri.to_string();
            tokio::spawn(async move {
                handle_inbound_repost_create(
                    &pool2,
                    &queue2,
                    &http2,
                    &hub2,
                    &did,
                    &at_uri,
                    &subject_uri,
                    created_at,
                )
                .await;
            });
        }

        "app.bsky.feed.like" => {
            match commit.operation.as_str() {
                "create" => {
                    let Some(record) = commit.record else {
                        return Ok(());
                    };
                    let Some(subject_uri) = record
                        .get("subject")
                        .and_then(|s| s.get("uri"))
                        .and_then(|v| v.as_str())
                    else {
                        return Ok(());
                    };
                    let emoji = record
                        .get("emoji")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    // 自分自身がローカルAPI経由でコミットしたLikeなら、非標準拡張フィールドとして
                    // 元の reactions.id が載っている（`encode_bsky_feed_like`）。これが戻ってきた
                    // 場合、ローカル即時通知と同じ reaction_id を通知に持たせることで、
                    // `notifications.reaction_id` の UNIQUE 制約により二重通知を防げる。
                    let seiran_reaction_id =
                        record.get("seiranReactionId").and_then(|v| v.as_i64());

                    let at_uri = format!("at://{}/app.bsky.feed.like/{}", did, commit.rkey);
                    let subject_uri = subject_uri.to_string();
                    let pool2 = pool.clone();
                    let http2 = Arc::clone(http);
                    let hub2 = Arc::clone(stream_hub);
                    tokio::spawn(async move {
                        handle_inbound_like_create(
                            &pool2,
                            &http2,
                            &hub2,
                            &did,
                            &at_uri,
                            &subject_uri,
                            emoji.as_deref(),
                            seiran_reaction_id,
                        )
                        .await;
                    });
                }
                "delete" => {
                    let at_uri = format!("at://{}/app.bsky.feed.like/{}", did, commit.rkey);
                    let pool2 = pool.clone();
                    let hub2 = Arc::clone(stream_hub);
                    tokio::spawn(async move {
                        handle_inbound_like_delete(&pool2, &hub2, &at_uri).await;
                    });
                }
                _ => {}
            }
        }

        _ => {}
    }

    Ok(())
}

/// Jetstream イベントから得た投稿本体を DB に保存し、ローカルフォロワーへ配信する。
/// Jetstream はほぼリアルタイムでレコード本体を同梱してくるため、AppView への
/// 再取得・インデックス遅延リトライは不要（旧 Relay Firehose 直結実装にはあった）。
#[allow(clippy::too_many_arguments)]
async fn save_bsky_post(
    pool: &PgPool,
    job_queue: &Arc<dyn JobQueue>,
    http: &Arc<reqwest::Client>,
    stream_hub: &StreamHub,
    at_uri: &str,
    author_did: &str,
    at_cid: &str,
    text: &str,
    mention_facets: &JsonValue,
    emoji_map: &JsonValue,
    created_at: chrono::DateTime<chrono::Utc>,
    actor_id: i64,
    username: &str,
    display_name: Option<&str>,
    avatar_url: Option<&str>,
    reply_to_post_id: Option<i64>,
    quote_of_post_id: Option<i64>,
    attachments: Vec<ParsedAttachment>,
    link_card: Option<ParsedLinkCard>,
) {
    let reply_id_str = reply_to_post_id.map(|id| id.to_string());
    let post_id = generate_snowflake_id(created_at);

    let result = sqlx::query(
        "INSERT INTO posts (id, actor_id, body, at_uri, at_cid, created_at, reply_to_post_id, mention_facets, emoji_map, quote_of_post_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (at_uri) DO NOTHING",
    )
    .bind(post_id)
    .bind(actor_id)
    .bind(text)
    .bind(at_uri)
    .bind(at_cid)
    .bind(created_at)
    .bind(reply_to_post_id)
    .bind(mention_facets)
    .bind(emoji_map)
    .bind(quote_of_post_id)
    .execute(pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() == 0 => {
            tracing::warn!("[Jetstream] 重複スキップ: {}", at_uri);
        }
        Ok(_) => {
            tracing::info!("[Jetstream] 保存完了: {}", at_uri);

            // 返信許可（threadgate）・引用可否（postgate）を取得して保存する
            // （#返信/引用グレーアウト、`docs/protocols.md`参照）。取得失敗時は両方とも
            // 「制限なし」のまま（誤ってボタンをグレーアウトしない）。
            let (reply_allow, quote_disabled) =
                seiran_common::atp::fetch_bsky_gates(http, author_did, at_uri).await;
            if let Err(e) = sqlx::query(
                "UPDATE posts SET bsky_reply_allow = $1, bsky_quote_disabled = $2 WHERE id = $3",
            )
            .bind(&reply_allow)
            .bind(quote_disabled)
            .bind(post_id)
            .execute(pool)
            .await
            {
                tracing::error!("[Jetstream] gate情報保存失敗（スキップ）: {}", e);
            }

            // URLカード（Bskyは常に最大1件、position=0固定）。埋め込みプレーヤーのiframe src
            // （oEmbed discovery）はここでは未解決のため、後追いでJob::LinkCardEmbedResolveへ
            // 委ねる（Bskyのexternal embedにはiframe情報が無いため）。
            if let Some(card) = &link_card {
                let result = sqlx::query(
                    "INSERT INTO post_link_cards (post_id, position, url, title, description, thumbnail_url)
                     VALUES ($1, 0, $2, $3, $4, $5)",
                )
                .bind(post_id)
                .bind(&card.url)
                .bind(&card.title)
                .bind(&card.description)
                .bind(card.thumbnail_url.as_deref())
                .execute(pool)
                .await;
                match result {
                    Ok(_) => {
                        if let Err(e) = job_queue
                            .enqueue(
                                Job::LinkCardEmbedResolve {
                                    post_id,
                                    position: 0,
                                    url: card.url.clone(),
                                },
                                priority::LOW,
                            )
                            .await
                        {
                            tracing::error!("[Jetstream] LinkCardEmbedResolve enqueue失敗: {}", e);
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "[Jetstream] post_link_cards INSERT失敗（投稿自体は成功済み）: {}",
                            e
                        );
                    }
                }
            }

            if let Err(e) = PgHashtagRepository::new(pool.clone())
                .link_post(post_id, text)
                .await
            {
                tracing::error!(
                    "[Jetstream] ハッシュタグ抽出・リンク失敗（投稿自体は成功済み）: {}",
                    e
                );
            }

            // リプライ通知: リプライ先がローカルユーザーの投稿であれば通知を作る（自己リプライは除く）。
            if let Some(parent_id) = reply_to_post_id {
                let parent_local_actor_id: Option<i64> = sqlx::query(
                    "SELECT p.actor_id FROM posts p JOIN actors a ON a.id = p.actor_id WHERE p.id = $1 AND a.actor_type = 'local'",
                )
                .bind(parent_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()
                .and_then(|row| row.try_get::<i64, _>("actor_id").ok());
                if let Some(parent_actor_id) = parent_local_actor_id.filter(|id| *id != actor_id) {
                    stream_hub.publish_event(
                        HashSet::from([parent_actor_id]),
                        "reply",
                        serde_json::json!({
                            "postId": post_id.to_string(),
                            "actor": { "username": username, "domain": serde_json::Value::Null, "displayName": display_name },
                        }),
                    );
                    let notif_id = generate_snowflake_id(chrono::Utc::now());
                    if let Err(e) = PgNotificationRepository::new(pool.clone())
                        .insert(
                            notif_id,
                            parent_actor_id,
                            NotificationKind::Reply,
                            Some(actor_id),
                            Some(post_id),
                            None,
                            None,
                            None,
                            None,
                            None,
                        )
                        .await
                    {
                        tracing::error!("[Jetstream] reply notifications INSERT 失敗: {}", e);
                    }
                }
            }

            // 引用通知: 引用先がローカルユーザーの投稿なら、フォロー関係にかかわらず通知する。
            if let Some(quoted_id) = quote_of_post_id {
                let quoted_local_actor_id: Option<i64> = sqlx::query(
                    "SELECT p.actor_id FROM posts p JOIN actors a ON a.id = p.actor_id
                     WHERE p.id = $1 AND a.actor_type = 'local'",
                )
                .bind(quoted_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()
                .and_then(|row| row.try_get::<i64, _>("actor_id").ok());
                if let Some(quoted_actor_id) = quoted_local_actor_id.filter(|id| *id != actor_id) {
                    stream_hub.publish_event(
                        HashSet::from([quoted_actor_id]),
                        "quote",
                        serde_json::json!({
                            "postId": post_id.to_string(),
                            "actor": { "username": username, "domain": serde_json::Value::Null, "displayName": display_name },
                        }),
                    );
                    let notif_id = generate_snowflake_id(chrono::Utc::now());
                    if let Err(e) = PgNotificationRepository::new(pool.clone())
                        .insert(
                            notif_id,
                            quoted_actor_id,
                            NotificationKind::Quote,
                            Some(actor_id),
                            Some(post_id),
                            None,
                            None,
                            Some(at_uri),
                            None,
                            None,
                        )
                        .await
                    {
                        tracing::error!("[Jetstream] quote notifications INSERT 失敗: {}", e);
                    }
                }
            }

            // メンション通知: mention_facets の各 did がローカルアクターを指す場合、通知を作る。
            // source_uri は渡さない（1投稿に複数の宛先がありうるため、投稿の at_uri を
            // 共有すると2人目以降が部分UNIQUEインデックスで弾かれてしまう。posts 自体は
            // at_uri の ON CONFLICT で既に重複排除済みのため、このブロックへの到達自体が
            // 新規保存時のみに限られ、重複INSERT対策は不要）。
            if let JsonValue::Array(spans) = mention_facets {
                let actor_repo = PgActorRepository::new(pool.clone());
                let notifications_repo = PgNotificationRepository::new(pool.clone());
                let mut notified: HashSet<i64> = HashSet::new();
                for span in spans {
                    let Some(mentioned_did) = span.get("did").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    if let Ok(Some(mentioned_actor)) = actor_repo.find_by_did(mentioned_did).await {
                        if mentioned_actor.actor_type != "local" || mentioned_actor.id == actor_id {
                            continue;
                        }
                        if !notified.insert(mentioned_actor.id) {
                            continue;
                        }
                        stream_hub.publish_event(
                            HashSet::from([mentioned_actor.id]),
                            "mention",
                            serde_json::json!({
                                "postId": post_id.to_string(),
                                "actor": { "username": username, "domain": serde_json::Value::Null, "displayName": display_name },
                            }),
                        );
                        let notif_id = generate_snowflake_id(chrono::Utc::now());
                        if let Err(e) = notifications_repo
                            .insert(
                                notif_id,
                                mentioned_actor.id,
                                NotificationKind::Mention,
                                Some(actor_id),
                                Some(post_id),
                                None,
                                None,
                                None,
                                None,
                                None,
                            )
                            .await
                        {
                            tracing::error!("[Jetstream] mention notifications INSERT 失敗: {}", e);
                        }
                    }
                }
            }

            // 添付（画像・動画）を post_attachments に保存
            if !attachments.is_empty() {
                let posts_repo = PgPostRepository::new(pool.clone());
                for (position, att) in attachments.iter().enumerate() {
                    if let Err(e) = posts_repo
                        .attach_remote_media_url(
                            post_id,
                            &att.url,
                            Some(&att.mime_type),
                            att.thumbnail_url.as_deref(),
                            false,
                            att.is_gif,
                            position as i16,
                        )
                        .await
                    {
                        tracing::error!("[Jetstream] 添付 URL 保存失敗（スキップ）: {}", e);
                    }
                }
            }

            // タイムラインチャンネル（homeTimeline/hybridTimeline/userList/hashtag。Bsky投稿は
            // is_local=falseのためlocalTimeline/globalTimelineには載らない）へ WebSocket 配信。
            // リプライの場合、リプライ先投稿者もフォロー中（または本人）のフォロワーのみに絞り込む
            // （`post_reply_target_followed`、REST の home_timeline/social_timeline や
            // `FollowRepository::find_home_recipient_ids` と同じ判定を共有するDB関数）。
            let home_recipients: HashSet<i64> = sqlx::query_scalar::<_, i64>(
                "SELECT f.follower_actor_id FROM follows f
                 JOIN actors a ON a.id = f.follower_actor_id
                 WHERE f.target_actor_id = $1 AND f.status = 'accepted'
                   AND a.actor_type = 'local'
                   AND post_reply_target_followed(f.follower_actor_id, $2)",
            )
            .bind(actor_id)
            .bind(reply_to_post_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();

            let list_ids: HashSet<i64> = sqlx::query_scalar::<_, i64>(
                "SELECT list_id FROM list_members WHERE actor_id = $1",
            )
            .bind(actor_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();

            let hashtags: HashSet<String> = seiran_common::hashtag::extract_hashtags(text)
                .into_iter()
                .collect();

            let attachments_json: Vec<JsonValue> = attachments
                .iter()
                .map(|att| {
                    serde_json::json!({
                        "url": att.url,
                        "mimeType": att.mime_type,
                        "width": att.width,
                        "height": att.height,
                        "thumbnailUrl": att.thumbnail_url,
                    })
                })
                .collect();
            let note_json = serde_json::json!({
                "id": post_id.to_string(),
                "text": text,
                "createdAt": created_at.to_rfc3339(),
                "user": {
                    "id": actor_id,
                    "username": username,
                    "domain": serde_json::Value::Null,
                    "displayName": display_name,
                    "actorType": "bsky",
                    "avatarUrl": avatar_url,
                },
                "attachments": attachments_json,
                "replyId": reply_id_str,
            });
            let scope = ChannelScope {
                is_local: false,
                visibility: "public".to_string(),
                home_recipients: Arc::new(home_recipients),
                list_ids: Arc::new(list_ids),
                hashtags: Arc::new(hashtags),
            };
            stream_hub.publish_channel_note(scope, note_json);
        }
        Err(e) => tracing::error!("[Jetstream] DB 保存失敗: {}", e),
    }
}

// ─── リポスト取り込み（app.bsky.feed.repost）───────────────────────────────

/// Bskyユーザーのリポストをタイムライン投稿として保存する（Fediverseの`Announce`受信
/// （`handle_announce`、`crates/seiran-common/src/jobs/inbound_activity_process.rs`）と対称の処理）。
/// リポスト対象がDBに未存在（Jetstreamの`wantedDids`絞り込みで取り込んでいなかった投稿等）
/// なら AppView から直接フェッチして保存する。対象がローカル投稿の場合は通知も作る。
#[allow(clippy::too_many_arguments)]
async fn handle_inbound_repost_create(
    pool: &PgPool,
    job_queue: &Arc<dyn JobQueue>,
    http: &reqwest::Client,
    stream_hub: &StreamHub,
    did: &str,
    at_uri: &str,
    subject_uri: &str,
    created_at: chrono::DateTime<chrono::Utc>,
) {
    let post_repo = PgPostRepository::new(pool.clone());

    let actor_id = match resolve_or_upsert_bsky_actor(pool, http, did).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("[Jetstream/Repost] reposter アクター解決失敗: {}", e);
            return;
        }
    };

    let repost_of_post_id = match post_repo.find_id_by_at_uri(subject_uri).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            tracing::info!(
                "[Jetstream/Repost] 対象ポストが DB に未存在。AppViewからフェッチします: {}",
                subject_uri
            );
            match seiran_common::atp::fetch_single_bsky_post(http, subject_uri).await {
                Ok(Some(post)) => {
                    let author_id =
                        match resolve_or_upsert_bsky_actor(pool, http, &post.author_did).await {
                            Ok(id) => id,
                            Err(e) => {
                                tracing::error!("[Jetstream/Repost] 対象ポスト著者解決失敗: {}", e);
                                return;
                            }
                        };
                    match seiran_common::atp::upsert_bsky_post(
                        pool, job_queue, http, author_id, &post,
                    )
                    .await
                    {
                        Ok(id) => id,
                        Err(e) => {
                            tracing::error!("[Jetstream/Repost] 対象ポスト保存失敗: {}", e);
                            return;
                        }
                    }
                }
                Ok(None) => {
                    tracing::warn!(
                        "[Jetstream/Repost] 対象ポストのフェッチに失敗（存在しない/削除済み): {}",
                        subject_uri
                    );
                    return;
                }
                Err(e) => {
                    tracing::error!("[Jetstream/Repost] 対象ポストのフェッチに失敗: {}", e);
                    return;
                }
            }
        }
        Err(e) => {
            tracing::error!("[Jetstream/Repost] 対象ポスト検索失敗: {}", e);
            return;
        }
    };

    // 重複チェック（同一アクターによる同一ポストのリポスト。再接続時のバックフィル重複等）
    if post_repo
        .find_repost_undo_info(actor_id, repost_of_post_id)
        .await
        .unwrap_or(None)
        .is_some()
    {
        return;
    }

    let post_id = generate_snowflake_id(created_at);
    if let Err(e) = post_repo
        .insert_repost_bsky(post_id, actor_id, at_uri, repost_of_post_id, created_at)
        .await
    {
        tracing::error!("[Jetstream/Repost] リポスト挿入失敗: {}", e);
        return;
    }

    // リポスト通知: リモート Bsky ユーザーがローカルユーザーの投稿をリポストした場合のみ作る。
    match post_repo.find_delivery_meta(repost_of_post_id).await {
        Ok(Some(meta)) if meta.actor_type == "local" && meta.actor_id != actor_id => {
            let actor_repo = PgActorRepository::new(pool.clone());
            if let Ok(Some(reposter)) = actor_repo.find_by_id(actor_id).await {
                stream_hub.publish_event(
                    HashSet::from([meta.actor_id]),
                    "repost",
                    serde_json::json!({
                        "postId": post_id.to_string(),
                        "actor": { "username": reposter.username, "domain": reposter.domain, "displayName": reposter.display_name },
                    }),
                );
            }
            let notif_id = generate_snowflake_id(chrono::Utc::now());
            if let Err(e) = PgNotificationRepository::new(pool.clone())
                .insert(
                    notif_id,
                    meta.actor_id,
                    NotificationKind::Repost,
                    Some(actor_id),
                    Some(post_id),
                    None,
                    None,
                    Some(at_uri),
                    None,
                    None,
                )
                .await
            {
                tracing::error!("[Jetstream/Repost] notifications INSERT 失敗: {}", e);
            }
        }
        Ok(_) => {}
        Err(e) => tracing::error!("[Jetstream/Repost] 元ポストメタ情報の取得に失敗: {}", e),
    }

    tracing::info!(
        "[Jetstream/Repost] リポスト保存完了: id={}, actor_id={}, repost_of={}",
        post_id,
        actor_id,
        repost_of_post_id
    );
}

// ─── リアクション連携（app.bsky.feed.like）────────────────────────────────

/// ATP Like（`app.bsky.feed.like`）の作成を検知した際の処理。
/// `subject_uri` がローカル投稿の `at_uri` と一致する場合のみ `reactions` へ INSERT し、
/// 通知ベル用イベント（著者のみ）とリアルタイム更新（`noteUpdated`、著者+フォロワー）を送出する。
/// `seiran_reaction_id` は Like レコードの非標準拡張フィールド（`seiranReactionId`）から
/// 抽出した値。自分自身がローカルAPI経由でコミットしたLikeが自分のfirehose受信で戻ってきた
/// ケースでは、ローカル即時通知insertと同じ `reactions.id` が入っており、
/// `notifications.reaction_id` の UNIQUE 制約で二重通知を防げる（`docs/protocols.md` 8節）。
#[allow(clippy::too_many_arguments)]
async fn handle_inbound_like_create(
    pool: &PgPool,
    http: &reqwest::Client,
    stream_hub: &StreamHub,
    did: &str,
    at_uri: &str,
    subject_uri: &str,
    emoji: Option<&str>,
    seiran_reaction_id: Option<i64>,
) {
    let posts_repo = PgPostRepository::new(pool.clone());
    let (post_id, post_author_id) = match posts_repo.find_id_and_actor_by_at_uri(subject_uri).await
    {
        Ok(Some(pair)) => pair,
        Ok(None) => return, // ローカル投稿ではない（あるいは未取り込み）
        Err(e) => {
            tracing::error!("[Jetstream/Like] 対象ポスト検索失敗: {}", e);
            return;
        }
    };

    let actor_id = match resolve_or_upsert_bsky_actor(pool, http, did).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("[Jetstream/Like] liker アクター解決失敗: {}", e);
            return;
        }
    };

    // ATP は「1投稿1いいね」が前提（Like レコード自体が unique）なので content は
    // 常に絵文字1個。emoji フィールドが無ければ ❤️（絵文字ピッカーと同じ、VS16付きハート）として扱う。
    let content = emoji.unwrap_or("❤️");

    // `content` がカスタム絵文字（`:shortcode:`）の場合、このサーバーの custom_emojis から
    // 画像 URL を解決する。これはローカルユーザーが自分で送ったカスタム絵文字リアクションが
    // ATP へ commit_like された後、自分自身の firehose 受信でここに戻ってくるケースに対応するため
    // （`ON CONFLICT (post_id, actor_id) DO UPDATE` で emoji_url も上書きされるため、ここで
    // `None` を渡すと `create_reaction` が設定した正しい URL を消してしまう回帰があった）。
    let emoji_url = match parse_reaction_shortcode_and_host(content).map(|(shortcode, _)| shortcode)
    {
        Some(shortcode) => {
            let emojis_repo = PgEmojiRepository::new(pool.clone());
            emojis_repo
                .find_url_by_shortcode(shortcode)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!("[Jetstream/Like] 絵文字URL解決失敗: {}", e);
                    None
                })
        }
        None => None,
    };

    let reactions_repo = PgReactionRepository::new(pool.clone());
    let new_reaction_id = generate_snowflake_id(chrono::Utc::now());
    if let Err(e) = reactions_repo
        .insert(
            new_reaction_id,
            post_id,
            actor_id,
            "like",
            content,
            None,
            Some(at_uri),
            emoji_url.as_deref(),
        )
        .await
    {
        tracing::error!("[Jetstream/Like] reactions INSERT 失敗: {}", e);
        return;
    }

    tracing::info!(
        "[Jetstream/Like] post {} に {} を記録（did={}）",
        post_id,
        content,
        did
    );

    // 通知ベル用（#37）: 自作自演（本尊が自分の投稿を Bsky 側からもいいねした等）は通知しない
    if post_author_id != actor_id {
        let actor_repo = PgActorRepository::new(pool.clone());
        if let Ok(Some(liker)) = actor_repo.find_by_id(actor_id).await {
            stream_hub.publish_event(
                HashSet::from([post_author_id]),
                "reaction",
                serde_json::json!({
                    "postId": post_id.to_string(),
                    "emoji": content,
                    "actor": { "username": liker.username, "domain": liker.domain, "displayName": liker.display_name },
                }),
            );
        }
        let notifications_repo = PgNotificationRepository::new(pool.clone());
        let notif_id = generate_snowflake_id(chrono::Utc::now());
        if let Err(e) = notifications_repo
            .insert(
                notif_id,
                post_author_id,
                NotificationKind::Reaction,
                Some(actor_id),
                Some(post_id),
                Some(content),
                None,
                Some(at_uri),
                seiran_reaction_id,
                None,
            )
            .await
        {
            tracing::error!("[Jetstream/Like] notifications INSERT 失敗: {}", e);
        }
    }

    let follows_repo = PgFollowRepository::new(pool.clone());
    broadcast_reaction_update(
        stream_hub,
        &follows_repo,
        &reactions_repo,
        post_id,
        post_author_id,
        actor_id,
        Some(content),
    )
    .await;
}

/// ATP Like（`app.bsky.feed.like`）の削除（Unlike）を検知した際の処理。
async fn handle_inbound_like_delete(pool: &PgPool, stream_hub: &StreamHub, at_uri: &str) {
    let reactions_repo = PgReactionRepository::new(pool.clone());
    let deleted = match reactions_repo.delete_by_at_uri(at_uri).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("[Jetstream/Unlike] reactions DELETE 失敗: {}", e);
            return;
        }
    };
    let Some((post_id, actor_id)) = deleted else {
        return; // 元々知らないリアクションだった（重複 delete イベント等）
    };

    tracing::info!(
        "[Jetstream/Unlike] post {} のリアクションを取消（at_uri={}）",
        post_id,
        at_uri
    );

    let posts_repo = PgPostRepository::new(pool.clone());
    let post_author_id = match posts_repo.find_by_id(post_id).await {
        Ok(Some(p)) => p.actor_id,
        _ => return,
    };

    let follows_repo = PgFollowRepository::new(pool.clone());
    broadcast_reaction_update(
        stream_hub,
        &follows_repo,
        &reactions_repo,
        post_id,
        post_author_id,
        actor_id,
        None,
    )
    .await;
}

/// ATP 投稿（`app.bsky.feed.post`）の削除を検知した際の処理。取り込み済み（`at_uri` 保存済み）の
/// 投稿のみ論理削除する。`did`はJetstreamのcommitのリポジトリ所有者そのものなので、Likeの削除と
/// 同様になりすまし確認は不要（`at_uri`自体がdidから組み立てられており、他者のdidの投稿を指せない）。
async fn handle_inbound_post_delete(pool: &PgPool, at_uri: &str) {
    let posts_repo = PgPostRepository::new(pool.clone());
    match posts_repo.soft_delete_by_at_uri(at_uri).await {
        Ok(Some((post_id, _actor_id))) => {
            tracing::info!("[Jetstream] 投稿 {} を削除（at_uri={}）", post_id, at_uri);
        }
        Ok(None) => {
            // 元々取り込んでいない投稿だった（フォロー対象外だった等）
        }
        Err(e) => {
            tracing::error!("[Jetstream] posts (delete) UPDATE 失敗: {}", e);
        }
    }
}

/// DID からローカル `actors` 行を解決する。無ければ AppView からプロフィールを取得して upsert する
/// （AP 側 `upsert_remote_fedi_actor` の ATP 版）。
pub(crate) async fn resolve_or_upsert_bsky_actor(
    pool: &PgPool,
    http: &reqwest::Client,
    did: &str,
) -> Result<i64, String> {
    let actor_repo = PgActorRepository::new(pool.clone());
    if let Ok(Some(actor)) = actor_repo.find_by_did(did).await {
        return Ok(actor.id);
    }

    let profile = fetch_bsky_profile(http, did).await?;
    let new_id = generate_snowflake_id(chrono::Utc::now());
    actor_repo
        .upsert_remote_bsky(
            new_id,
            did,
            &profile.handle,
            profile.display_name.as_deref(),
            profile.avatar.as_deref(),
            chrono::Utc::now(),
        )
        .await
        .map_err(|e| format!("upsert_remote_bsky 失敗: {}", e))
}

#[cfg(test)]
mod facet_tests {
    use super::*;

    #[test]
    fn jetstream_subscription_includes_reposts() {
        let url = build_jetstream_url(None, &[]);
        assert!(url.contains("wantedCollections=app.bsky.feed.repost"));
    }
}

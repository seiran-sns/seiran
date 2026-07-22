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

use serde::Deserialize;
use serde_json::Value as JsonValue;
use sqlx::{PgPool, Row};
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::StreamExt;

use seiran_common::atp::fetch_bsky_profile;
use seiran_common::jetstream_control::fetch_wanted_dids_touch;
use seiran_common::jetstream_leader::{self, JetstreamLeaderElector};
use seiran_common::repository::{
    ActorRepository, HashtagRepository, NotificationKind, NotificationRepository, PostRepository, ReactionRepository,
    PgActorRepository, PgFollowRepository, PgHashtagRepository, PgNotificationRepository, PgPostRepository, PgReactionRepository,
};
use seiran_common::streaming::broadcast_reaction_update;
use seiran_common::traits::{Job, JobQueue};
use seiran_common::{generate_snowflake_id, StreamHub};

const JETSTREAM_BASE_URL: &str =
    "wss://jetstream1.us-east.bsky.network/subscribe?wantedCollections=app.bsky.feed.post&wantedCollections=app.bsky.feed.like";

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
#[allow(clippy::too_many_arguments)]
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
                    match JetstreamLeaderElector::connect(url, jetstream_leader::DEFAULT_LEADER_KEY).await {
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
                tracing::info!("[Jetstream] リーダーに昇格（またはRedis未使用の単独運用）。接続開始。");
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
                tracing::error!("[Jetstream] エラー: {}。{}秒後に再接続します。", e, backoff_secs);
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
        Ok(rows) => rows.iter().filter_map(|r| r.try_get::<String, _>("did").ok()).collect(),
        Err(e) => {
            tracing::error!("[Jetstream] wantedDids取得失敗（無絞り込みで接続します）: {}", e);
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
    tracing::info!("[Jetstream] 接続中（wantedDids {}件）: {}", wanted_dids.len(), url);

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

/// Bsky embed（画像・動画）から復元した添付情報。CDN URL は DID + blob CID のみから
/// 決定的に組み立てられる（Bluesky AppView への追加問い合わせは不要）。
struct ParsedAttachment {
    url: String,
    mime_type: String,
    width: i32,
    height: i32,
    thumbnail_url: Option<String>,
}

/// AP Note の `record.embed` を解析し、添付URL一覧を組み立てる。
/// `app.bsky.embed.images` → `https://cdn.bsky.app/img/feed_fullsize/plain/{did}/{cid}`
/// `app.bsky.embed.video` → HLSプレイリスト `https://video.bsky.app/watch/{did}/{cid}/playlist.m3u8`
///   （動画本体はBluesky公式の動画処理パイプラインでHLSにトランスコードされて配信されるため、
///   PDS上のblob自体を指すURLではなくこの固定パターンを使う。サムネイルも同様のパターン）。
/// `app.bsky.embed.recordWithMedia`（引用+メディア）は `media` フィールドを再帰的に見る。
/// 未知の embed 種別や画像/動画以外（`external`/`record` 単体等）は空を返す。
fn parse_bsky_embed_attachments(embed: &JsonValue, did: &str) -> Vec<ParsedAttachment> {
    let embed_type = embed.get("$type").and_then(|v| v.as_str()).unwrap_or("");
    match embed_type {
        "app.bsky.embed.images" => {
            embed.get("images")
                .and_then(|v| v.as_array())
                .map(|images| {
                    images.iter().filter_map(|img| {
                        let cid = img.get("image")?.get("ref")?.get("$link")?.as_str()?;
                        let mime_type = img.get("image")
                            .and_then(|i| i.get("mimeType"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("image/jpeg")
                            .to_string();
                        let width = img.get("aspectRatio").and_then(|a| a.get("width")).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                        let height = img.get("aspectRatio").and_then(|a| a.get("height")).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                        let url = format!("https://cdn.bsky.app/img/feed_fullsize/plain/{}/{}", did, cid);
                        Some(ParsedAttachment { url, mime_type, width, height, thumbnail_url: None })
                    }).collect()
                })
                .unwrap_or_default()
        }
        "app.bsky.embed.video" => {
            let Some(cid) = embed.get("video").and_then(|v| v.get("ref")).and_then(|r| r.get("$link")).and_then(|v| v.as_str()) else {
                return vec![];
            };
            let width = embed.get("aspectRatio").and_then(|a| a.get("width")).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let height = embed.get("aspectRatio").and_then(|a| a.get("height")).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let did_encoded = urlencoding::encode(did);
            let url = format!("https://video.bsky.app/watch/{}/{}/playlist.m3u8", did_encoded, cid);
            let thumbnail_url = format!("https://video.bsky.app/watch/{}/{}/thumbnail.jpg", did_encoded, cid);
            vec![ParsedAttachment {
                url, mime_type: "application/vnd.apple.mpegurl".to_string(), width, height,
                thumbnail_url: Some(thumbnail_url),
            }]
        }
        "app.bsky.embed.recordWithMedia" => {
            embed.get("media")
                .map(|media| parse_bsky_embed_attachments(media, did))
                .unwrap_or_default()
        }
        _ => vec![],
    }
}

/// `app.bsky.richtext.facet` の index（UTF-8 バイトオフセット）。
#[derive(Deserialize)]
struct JetstreamFacetIndex {
    #[serde(rename = "byteStart")]
    byte_start: usize,
    #[serde(rename = "byteEnd")]
    byte_end: usize,
}

/// facet の feature 種別（`$type` で判別）。未知の種別はパース全体を失敗させないよう
/// `Unknown` に落とす。
#[derive(Deserialize)]
#[serde(tag = "$type")]
enum JetstreamFacetFeature {
    #[serde(rename = "app.bsky.richtext.facet#link")]
    Link { uri: String },
    #[serde(rename = "app.bsky.richtext.facet#mention")]
    Mention { did: String },
    #[serde(rename = "app.bsky.richtext.facet#tag")]
    Tag,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct JetstreamFacet {
    index: JetstreamFacetIndex,
    features: Vec<JetstreamFacetFeature>,
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

/// mention facet 1件分の位置情報（本文は書き換えず、別途 `posts.mention_facets` へ
/// 保存する。ハンドルは可変なので表示時（`NoteResponse` 生成時）に都度解決する）。
struct MentionFacetSpan {
    byte_start: usize,
    byte_end: usize,
    did: String,
}

/// `#link` facet が示すテキスト範囲を、内部リンクマーカー `[表示テキスト](URL)`
/// （Markdownリンク記法）に書き換える。URL は不変なのでここで確定してよい。
///
/// `#mention` facet は本文を書き換えない（メンション先のハンドルは DID 解決状況や
/// ハンドル変更で変わりうるため、表示時に都度解決する方針。フロントの MFM 描画コンポーネント
/// が `@user@host` パターンを自動でプロフィールリンクに変換するので、Markdownリンクで
/// 包む必要も無い）。代わりに `(byteStart, byteEnd, did)` を戻り値として返す。
/// `#tag` facet も無変換（`#tag` は既に地の文にありフロント側の自動検出に委ねる）。
///
/// `byteStart`/`byteEnd` は他 PDS から届く未検証の値のため、範囲外・非文字境界・他 facet
/// との重なりはそのfacetだけスキップする（投稿保存自体を失敗させない）。DB アクセスを含まない
/// 純粋関数にしてあるため単体テストしやすい。
fn apply_link_facets(text: &str, facets: Vec<JetstreamFacet>) -> (String, Vec<MentionFacetSpan>) {
    if facets.is_empty() {
        return (text.to_string(), Vec::new());
    }

    // mention facet はテキストを変更しないため、位置情報だけ先に抜き出しておく
    // （範囲外・非文字境界のものは保存対象から除外）。
    let mut mention_spans = Vec::new();
    for facet in &facets {
        let start = facet.index.byte_start;
        let end = facet.index.byte_end;
        if start >= end || end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        for feature in &facet.features {
            if let JetstreamFacetFeature::Mention { did } = feature {
                mention_spans.push(MentionFacetSpan { byte_start: start, byte_end: end, did: did.clone() });
            }
        }
    }

    // 以降は #link facet のみを対象に、後ろから順に本文へ焼き込む。
    let mut link_facets: Vec<JetstreamFacet> = facets
        .into_iter()
        .filter(|f| f.features.iter().any(|feat| matches!(feat, JetstreamFacetFeature::Link { .. })))
        .collect();
    link_facets.sort_by_key(|f| std::cmp::Reverse(f.index.byte_start));

    let mut result = text.to_string();
    let mut upper_bound = result.len();

    for facet in link_facets {
        let start = facet.index.byte_start;
        let end = facet.index.byte_end;
        if start >= end || end > result.len() || end > upper_bound {
            continue;
        }
        if !result.is_char_boundary(start) || !result.is_char_boundary(end) {
            continue;
        }

        let Some(JetstreamFacetFeature::Link { uri }) =
            facet.features.into_iter().find(|f| matches!(f, JetstreamFacetFeature::Link { .. }))
        else {
            continue;
        };

        let original = result[start..end].to_string();
        let replacement = format!("[{}]({})", original, uri);
        result.replace_range(start..end, &replacement);
        upper_bound = start;
    }

    (result, mention_spans)
}

/// facet を本文へ適用する（`apply_link_facets` の DB/Job キュー連携込み版）。
/// 戻り値は `(link 適用済み本文, mention_facets の JSON 配列)`。
/// 未知 DID（ローカル `actors` に無い）は `Job::ResolveBskyMention` をキューに積んで
/// 非同期解決を促す（ベストエフォート。enqueue に失敗しても投稿保存は継続し、
/// 表示時の都度解決に委ねる）。
async fn apply_bsky_facets(
    pool: &PgPool,
    job_queue: &Arc<dyn JobQueue>,
    text: &str,
    facets: Vec<JetstreamFacet>,
) -> (String, JsonValue) {
    if facets.is_empty() {
        return (text.to_string(), JsonValue::Array(vec![]));
    }

    let (body, mention_spans) = apply_link_facets(text, facets);

    if !mention_spans.is_empty() {
        let actor_repo = PgActorRepository::new(pool.clone());
        let mut queued_dids = HashSet::new();
        for span in &mention_spans {
            if !queued_dids.insert(span.did.clone()) {
                continue;
            }
            let known = actor_repo.find_by_did(&span.did).await.ok().flatten().is_some();
            if known {
                continue;
            }
            if let Err(e) = job_queue
                .enqueue(
                    Job::ResolveBskyMention { did: span.did.clone() },
                    seiran_common::queue::worker::priority::NORMAL,
                )
                .await
            {
                tracing::warn!(
                    "[Jetstream] ResolveBskyMention enqueue失敗（次回表示時に再試行）: {}",
                    e
                );
            }
        }
    }

    let mention_facets_json = JsonValue::Array(
        mention_spans
            .iter()
            .map(|s| {
                serde_json::json!({
                    "byteStart": s.byte_start,
                    "byteEnd": s.byte_end,
                    "did": s.did,
                })
            })
            .collect(),
    );

    (body, mention_facets_json)
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
            let parsed_facets: Vec<JetstreamFacet> = record
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
            let attachments: Vec<ParsedAttachment> = record.get("embed")
                .map(|embed| parse_bsky_embed_attachments(embed, &did))
                .unwrap_or_default();

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
            let at_uri2 = at_uri.clone();
            let body_text = body_text.to_string();

            tokio::spawn(async move {
                let reply_to_post_id = match &reply_parent_uri {
                    Some(parent_uri) => {
                        let posts_repo = PgPostRepository::new(pool2.clone());
                        match posts_repo.find_id_and_actor_by_at_uri(parent_uri).await {
                            Ok(Some((parent_post_id, _))) => Some(parent_post_id),
                            Ok(None) => None,
                            Err(e) => {
                                tracing::error!("[Jetstream] リプライ親投稿検索失敗（通常投稿として保存）: {}", e);
                                None
                            }
                        }
                    }
                    None => None,
                };
                let (body_text, mention_facets) =
                    apply_bsky_facets(&pool2, &queue2, &body_text, parsed_facets).await;
                save_bsky_post(
                    &pool2, &hub2, &at_uri2, &cid, &body_text, &mention_facets, created_at,
                    actor_id, &username, display_name.as_deref(), avatar_url.as_deref(),
                    reply_to_post_id, attachments,
                ).await;
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
                    let emoji = record.get("emoji").and_then(|v| v.as_str()).map(|s| s.to_string());

                    let at_uri = format!("at://{}/app.bsky.feed.like/{}", did, commit.rkey);
                    let subject_uri = subject_uri.to_string();
                    let pool2 = pool.clone();
                    let http2 = Arc::clone(http);
                    let hub2 = Arc::clone(stream_hub);
                    tokio::spawn(async move {
                        handle_inbound_like_create(&pool2, &http2, &hub2, &did, &at_uri, &subject_uri, emoji.as_deref()).await;
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
    stream_hub: &StreamHub,
    at_uri: &str,
    at_cid: &str,
    text: &str,
    mention_facets: &JsonValue,
    created_at: chrono::DateTime<chrono::Utc>,
    actor_id: i64,
    username: &str,
    display_name: Option<&str>,
    avatar_url: Option<&str>,
    reply_to_post_id: Option<i64>,
    attachments: Vec<ParsedAttachment>,
) {
    let reply_id_str = reply_to_post_id.map(|id| id.to_string());
    let post_id = generate_snowflake_id(created_at);

    let result = sqlx::query(
        "INSERT INTO posts (id, actor_id, body, at_uri, at_cid, created_at, reply_to_post_id, mention_facets)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
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
    .execute(pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() == 0 => {
            tracing::warn!("[Jetstream] 重複スキップ: {}", at_uri);
        }
        Ok(_) => {
            tracing::info!("[Jetstream] 保存完了: {}", at_uri);

            if let Err(e) = PgHashtagRepository::new(pool.clone()).link_post(post_id, text).await {
                tracing::error!("[Jetstream] ハッシュタグ抽出・リンク失敗（投稿自体は成功済み）: {}", e);
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
                        .insert(notif_id, parent_actor_id, NotificationKind::Reply, Some(actor_id), Some(post_id), None, None, None)
                        .await
                    {
                        tracing::error!("[Jetstream] reply notifications INSERT 失敗: {}", e);
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
                    let Some(mentioned_did) = span.get("did").and_then(|v| v.as_str()) else { continue };
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
                            .insert(notif_id, mentioned_actor.id, NotificationKind::Mention, Some(actor_id), Some(post_id), None, None, None)
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
                    if let Err(e) = posts_repo.attach_remote_media_url(
                        post_id, &att.url, Some(&att.mime_type), att.thumbnail_url.as_deref(), position as i16,
                    ).await {
                        tracing::error!("[Jetstream] 添付 URL 保存失敗（スキップ）: {}", e);
                    }
                }
            }

            // ローカルフォロワー + この投稿者をリストに含めているリスト所有者へ WebSocket 配信
            // （リスト機能 #63: リストタブを開いている間もリアルタイム更新されるように）。
            let follower_rows = sqlx::query(
                "SELECT f.follower_actor_id AS recipient_id FROM follows f
                 JOIN actors a ON a.id = f.follower_actor_id
                 WHERE f.target_actor_id = $1 AND f.status = 'accepted'
                   AND a.actor_type = 'local'
                 UNION
                 SELECT l.owner_actor_id AS recipient_id FROM list_members lm
                 JOIN lists l ON l.id = lm.list_id
                 WHERE lm.actor_id = $1",
            )
            .bind(actor_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default();

            let recipients: HashSet<i64> = follower_rows
                .iter()
                .filter_map(|r| r.try_get::<i64, _>("recipient_id").ok())
                .collect();

            if !recipients.is_empty() {
                let attachments_json: Vec<JsonValue> = attachments.iter().map(|att| {
                    serde_json::json!({
                        "url": att.url,
                        "mimeType": att.mime_type,
                        "width": att.width,
                        "height": att.height,
                        "thumbnailUrl": att.thumbnail_url,
                    })
                }).collect();
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
                stream_hub.publish_note(recipients, &note_json);
            }
        }
        Err(e) => tracing::error!("[Jetstream] DB 保存失敗: {}", e),
    }
}

// ─── リアクション連携（app.bsky.feed.like）────────────────────────────────

/// ATP Like（`app.bsky.feed.like`）の作成を検知した際の処理。
/// `subject_uri` がローカル投稿の `at_uri` と一致する場合のみ `reactions` へ INSERT し、
/// 通知ベル用イベント（著者のみ）とリアルタイム更新（`noteUpdated`、著者+フォロワー）を送出する。
#[allow(clippy::too_many_arguments)]
async fn handle_inbound_like_create(
    pool: &PgPool,
    http: &reqwest::Client,
    stream_hub: &StreamHub,
    did: &str,
    at_uri: &str,
    subject_uri: &str,
    emoji: Option<&str>,
) {
    let posts_repo = PgPostRepository::new(pool.clone());
    let (post_id, post_author_id) = match posts_repo.find_id_and_actor_by_at_uri(subject_uri).await {
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
    let reactions_repo = PgReactionRepository::new(pool.clone());
    if let Err(e) = reactions_repo.insert(post_id, actor_id, "like", content, None, Some(at_uri), None).await {
        tracing::error!("[Jetstream/Like] reactions INSERT 失敗: {}", e);
        return;
    }

    tracing::info!("[Jetstream/Like] post {} に {} を記録（did={}）", post_id, content, did);

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
            .insert(notif_id, post_author_id, NotificationKind::Reaction, Some(actor_id), Some(post_id), Some(content), None, Some(at_uri))
            .await
        {
            tracing::error!("[Jetstream/Like] notifications INSERT 失敗: {}", e);
        }
    }

    let follows_repo = PgFollowRepository::new(pool.clone());
    broadcast_reaction_update(
        stream_hub, &follows_repo, &reactions_repo,
        post_id, post_author_id, actor_id, Some(content),
    ).await;
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

    tracing::info!("[Jetstream/Unlike] post {} のリアクションを取消（at_uri={}）", post_id, at_uri);

    let posts_repo = PgPostRepository::new(pool.clone());
    let post_author_id = match posts_repo.find_by_id(post_id).await {
        Ok(Some(p)) => p.actor_id,
        _ => return,
    };

    let follows_repo = PgFollowRepository::new(pool.clone());
    broadcast_reaction_update(
        stream_hub, &follows_repo, &reactions_repo,
        post_id, post_author_id, actor_id, None,
    ).await;
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
pub(crate) async fn resolve_or_upsert_bsky_actor(pool: &PgPool, http: &reqwest::Client, did: &str) -> Result<i64, String> {
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

    fn link_facet(byte_start: usize, byte_end: usize, uri: &str) -> JetstreamFacet {
        JetstreamFacet {
            index: JetstreamFacetIndex { byte_start, byte_end },
            features: vec![JetstreamFacetFeature::Link { uri: uri.to_string() }],
        }
    }

    fn mention_facet(byte_start: usize, byte_end: usize, did: &str) -> JetstreamFacet {
        JetstreamFacet {
            index: JetstreamFacetIndex { byte_start, byte_end },
            features: vec![JetstreamFacetFeature::Mention { did: did.to_string() }],
        }
    }

    #[test]
    fn single_link_facet_becomes_markdown_link() {
        let text = "見て example.com だよ";
        let byte_start = text.find("example.com").unwrap();
        let byte_end = byte_start + "example.com".len();
        let facets = vec![link_facet(byte_start, byte_end, "https://example.com")];
        let (result, mentions) = apply_link_facets(text, facets);
        assert_eq!(result, "見て [example.com](https://example.com) だよ");
        assert!(mentions.is_empty());
    }

    #[test]
    fn multiple_facets_applied_back_to_front_preserve_offsets() {
        let text = "foo.com and bar.com";
        let foo_start = text.find("foo.com").unwrap();
        let foo_end = foo_start + "foo.com".len();
        let bar_start = text.find("bar.com").unwrap();
        let bar_end = bar_start + "bar.com".len();
        // わざと昇順で渡し、関数側のソートが正しく後ろから処理することを確認する。
        let facets = vec![
            link_facet(foo_start, foo_end, "https://foo.com"),
            link_facet(bar_start, bar_end, "https://bar.com"),
        ];
        let (result, _) = apply_link_facets(text, facets);
        assert_eq!(
            result,
            "[foo.com](https://foo.com) and [bar.com](https://bar.com)"
        );
    }

    #[test]
    fn mention_facet_does_not_rewrite_body_but_is_extracted() {
        // メンションは本文を書き換えない（ハンドルは可変なので表示時に都度解決する）。
        // 呼び出し側が byteStart/byteEnd/did を mention_facets として保存できるよう返す。
        let text = "hi @alice.bsky.social and @unknown.bsky.social";
        let alice_start = text.find("@alice.bsky.social").unwrap();
        let alice_end = alice_start + "@alice.bsky.social".len();
        let unknown_start = text.find("@unknown.bsky.social").unwrap();
        let unknown_end = unknown_start + "@unknown.bsky.social".len();
        let facets = vec![
            mention_facet(alice_start, alice_end, "did:plc:alice"),
            mention_facet(unknown_start, unknown_end, "did:plc:unknown"),
        ];
        let (result, mentions) = apply_link_facets(text, facets);
        assert_eq!(result, text, "mention facet は本文を変更しない");
        assert_eq!(mentions.len(), 2);
        assert_eq!(mentions[0].did, "did:plc:alice");
        assert_eq!(mentions[0].byte_start, alice_start);
        assert_eq!(mentions[0].byte_end, alice_end);
        assert_eq!(mentions[1].did, "did:plc:unknown");
    }

    #[test]
    fn tag_only_facet_is_left_unchanged() {
        let text = "#rust最高";
        let byte_end = "#rust".len();
        let facets = vec![JetstreamFacet {
            index: JetstreamFacetIndex { byte_start: 0, byte_end },
            features: vec![JetstreamFacetFeature::Tag],
        }];
        let (result, mentions) = apply_link_facets(text, facets);
        assert_eq!(result, text);
        assert!(mentions.is_empty());
    }

    #[test]
    fn out_of_range_facet_is_skipped_without_panicking() {
        let text = "short";
        let facets = vec![link_facet(0, 1000, "https://example.com")];
        let (result, _) = apply_link_facets(text, facets);
        assert_eq!(result, text);
    }

    #[test]
    fn non_char_boundary_facet_is_skipped_without_panicking() {
        // "あ" は UTF-8 で3バイト。境界外の1バイト目を指定してもパニックしないこと。
        let text = "あいう";
        let facets = vec![link_facet(1, 2, "https://example.com")];
        let (result, _) = apply_link_facets(text, facets);
        assert_eq!(result, text);
    }

    #[test]
    fn overlapping_facets_second_one_is_skipped() {
        let text = "abcdef";
        // [0,4) と [2,6) が重なる。降順ソートで先に [2,6) が処理され、
        // 後続の [0,4) は upper_bound (=2) を超えるためスキップされる。
        let facets = vec![
            link_facet(0, 4, "https://a.example.com"),
            link_facet(2, 6, "https://b.example.com"),
        ];
        let (result, _) = apply_link_facets(text, facets);
        assert_eq!(result, "ab[cdef](https://b.example.com)");
    }

    #[test]
    fn out_of_range_mention_facet_is_dropped_without_panicking() {
        let text = "short";
        let facets = vec![mention_facet(0, 1000, "did:plc:x")];
        let (result, mentions) = apply_link_facets(text, facets);
        assert_eq!(result, text);
        assert!(mentions.is_empty());
    }
}

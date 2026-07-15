use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

use crate::atp::plc::{signing_key_from_pem, PlcError};
use crate::atp::repo::{
    build_account_frame, build_commit_frame, build_identity_frame, build_mst, cid_from_sha256_hex, cid_from_str,
    cid_to_string, create_commit, encode_car, encode_bsky_actor_profile, encode_bsky_feed_post,
    encode_bsky_feed_repost, encode_bsky_feed_like, encode_bsky_graph_follow,
    generate_tid, Cid, CommitEvtOp, RepoError, BskyFacet, BskyImage, BskyEmbed,
    BskyPostReply,
};

#[derive(Debug, thiserror::Error)]
pub enum AtpCommitError {
    #[error("DB エラー: {0}")]
    Db(#[from] sqlx::Error),
    #[error("ATP リポジトリエラー: {0}")]
    Repo(#[from] RepoError),
    #[error("PLC エラー: {0}")]
    Plc(#[from] PlcError),
    #[error("アクター設定不備: {0}")]
    ActorConfig(&'static str),
}

/// subscribeRepos WebSocket にブロードキャストするイベント
#[derive(Clone, Serialize, Deserialize)]
pub struct AtpCommitEvent {
    pub frame_bytes: Vec<u8>,
    #[allow(dead_code)]
    pub seq: i64,
}

/// Redis Pub/Sub でイベントをプロセス間配信する際のチャンネル名。
const ATP_EVENTS_CHANNEL: &str = "seiran:atp:events";

/// Redis Pub/Sub を購読し、受信したイベントをローカルの `event_tx` へ転送し続ける。
/// 接続が切れる・購読が終了すると `Err` を返し、呼び出し側が再接続をリトライする。
async fn run_redis_bridge_subscriber(
    client: &redis::Client,
    event_tx: &Arc<broadcast::Sender<AtpCommitEvent>>,
) -> Result<(), String> {
    let mut pubsub = client
        .get_async_pubsub()
        .await
        .map_err(|e| format!("Redis接続に失敗しました: {}", e))?;
    pubsub
        .subscribe(ATP_EVENTS_CHANNEL)
        .await
        .map_err(|e| format!("Redis購読に失敗しました: {}", e))?;

    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        let payload: String = match msg.get_payload() {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("[AtpCommitService] Redisメッセージの取得に失敗しました: {}", e);
                continue;
            }
        };
        match serde_json::from_str::<AtpCommitEvent>(&payload) {
            Ok(event) => {
                let _ = event_tx.send(event);
            }
            Err(e) => {
                tracing::error!("[AtpCommitService] イベントのデシリアライズに失敗しました: {}", e);
            }
        }
    }

    Err("購読ストリームが終了しました".to_string())
}

/// コミット処理に渡すレコード情報
pub struct CommitRecord {
    pub collection: &'static str,
    pub rkey: String,
    pub cbor: Vec<u8>,
    pub cid: Cid,
    pub action: &'static str,
    pub blob_cids: Vec<Cid>,
}

/// コミット処理の結果
pub struct CommitResult {
    pub commit_cid: Cid,
    pub rev: String,
    pub seq: i64,
    pub at_did: String,
}

pub struct AtpCommitService {
    pool: PgPool,
    event_tx: Arc<broadcast::Sender<AtpCommitEvent>>,
    http_client: Arc<reqwest::Client>,
    /// `Some` の場合、コミットイベントはローカルの `event_tx` へ直接送らず Redis 経由で
    /// 配信する（プロセス間配信ブリッジが有効。`with_redis_bridge` 参照）。
    redis_pub: Option<redis::aio::ConnectionManager>,
}

impl AtpCommitService {
    pub fn new(
        pool: PgPool,
        event_tx: Arc<broadcast::Sender<AtpCommitEvent>>,
        http_client: Arc<reqwest::Client>,
    ) -> Self {
        Self { pool, event_tx, http_client, redis_pub: None }
    }

    /// コミットイベントを Redis Pub/Sub 経由でプロセス間配信するブリッジを有効にする。
    ///
    /// `seiran-api` を複数レプリカで水平スケールした場合、レプリカ A で行われたコミットは
    /// レプリカ A の `event_tx`（プロセス内 broadcast）にしか流れず、レプリカ B の
    /// subscribeRepos WebSocket クライアントには届かない。このブリッジを有効にすると、
    /// コミット時にローカル送信の代わりに Redis へ publish し、全プロセス（自分自身を含む）が
    /// 購読タスク経由でそれぞれの `event_tx` に転送するため、どのレプリカに接続した
    /// WebSocket クライアントにも届くようになる。
    ///
    /// モノリスモード（`--role all`）や単一レプリカ運用では不要（`event_tx` の直接送信で十分）。
    pub async fn with_redis_bridge(&mut self, redis_url: &str) -> Result<(), String> {
        let client = redis::Client::open(redis_url)
            .map_err(|e| format!("Redis接続URLが不正です: {}", e))?;
        let publish_conn = redis::aio::ConnectionManager::new(client.clone())
            .await
            .map_err(|e| format!("Redis接続に失敗しました: {}", e))?;
        self.redis_pub = Some(publish_conn);

        let event_tx = Arc::clone(&self.event_tx);
        tokio::spawn(async move {
            loop {
                match run_redis_bridge_subscriber(&client, &event_tx).await {
                    Ok(()) => {}
                    Err(e) => tracing::info!("[AtpCommitService] Redis購読が切断されました: {}", e),
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
                tracing::info!("[AtpCommitService] Redis購読を再接続します...");
            }
        });

        Ok(())
    }

    pub fn event_tx(&self) -> &Arc<broadcast::Sender<AtpCommitEvent>> {
        &self.event_tx
    }

    /// コミットイベントを配信する。Redis ブリッジが有効なら Redis へ publish し
    /// （自プロセスへも購読タスク経由で戻ってくる）、無効ならローカル `event_tx` へ直接送る。
    fn publish_event(&self, event: AtpCommitEvent) {
        let Some(conn) = self.redis_pub.clone() else {
            let _ = self.event_tx.send(event);
            return;
        };
        tokio::spawn(async move {
            let mut conn = conn;
            let payload = match serde_json::to_string(&event) {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!("[AtpCommitService] イベントのシリアライズに失敗しました: {}", e);
                    return;
                }
            };
            if let Err(e) = conn.publish::<_, _, ()>(ATP_EVENTS_CHANNEL, payload).await {
                tracing::error!("[AtpCommitService] Redis publish に失敗しました: {}", e);
            }
        });
    }

    fn spawn_request_crawl(&self) {
        if let Ok(local_domain) = std::env::var("LOCAL_DOMAIN") {
            let http_client = Arc::clone(&self.http_client);
            tokio::spawn(async move {
                match http_client
                    .post("https://bsky.network/xrpc/com.atproto.sync.requestCrawl")
                    .json(&serde_json::json!({"hostname": local_domain}))
                    .send()
                    .await
                {
                    Ok(res) => tracing::info!("[atp] requestCrawl → {}", res.status()),
                    Err(e) => tracing::error!("[atp] requestCrawl 失敗: {}", e),
                }
            });
        }
    }

    /// 指定アクターの全 ATP レコードを MST 構築用エントリとしてロードする。
    async fn load_atp_entries(&self, actor_id: i64) -> Result<Vec<(String, Cid)>, AtpCommitError> {
        let post_rows = sqlx::query(
            "SELECT at_rkey, at_cid FROM posts
             WHERE actor_id = $1 AND at_rkey IS NOT NULL AND at_cid IS NOT NULL AND deleted_at IS NULL",
        )
        .bind(actor_id)
        .fetch_all(&self.pool)
        .await?;

        let record_rows = sqlx::query(
            "SELECT collection, rkey, cid FROM atp_records WHERE actor_id = $1",
        )
        .bind(actor_id)
        .fetch_all(&self.pool)
        .await?;

        let mut entries = Vec::new();
        for row in &post_rows {
            let rk: String = row.try_get("at_rkey")?;
            let cid_str: String = row.try_get("at_cid")?;
            let cid = cid_from_str(&cid_str)?;
            entries.push((format!("app.bsky.feed.post/{}", rk), cid));
        }
        for row in &record_rows {
            let col: String = row.try_get("collection")?;
            let rk: String = row.try_get("rkey")?;
            let cid_str: String = row.try_get("cid")?;
            let cid = cid_from_str(&cid_str)?;
            entries.push((format!("{}/{}", col, rk), cid));
        }
        Ok(entries)
    }

    /// 共通コミットパイプライン。
    /// MST 構築 → commit 署名 → CAR 生成 → atp_blocks 保存 → actors 更新
    /// → atp_records 保存 → atp_repo_events 記録 → WebSocket ブロードキャスト
    ///
    /// `post_id` を指定すると、同一トランザクション内で `posts.at_uri/at_cid/at_rkey` を更新する。
    async fn commit_record_inner(
        &self,
        actor_id: i64,
        record: CommitRecord,
        now: DateTime<Utc>,
        post_id: Option<i64>,
    ) -> Result<CommitResult, AtpCommitError> {
        // ① アクター情報取得
        let actor_row = sqlx::query(
            "SELECT at_did, at_signing_key_pem, at_repo_cid, at_repo_rev
             FROM actors WHERE id = $1",
        )
        .bind(actor_id)
        .fetch_one(&self.pool)
        .await?;

        let at_did: String = actor_row
            .try_get::<Option<String>, _>("at_did")?
            .ok_or(AtpCommitError::ActorConfig("at_did が未設定"))?;
        let signing_key_pem: String = actor_row
            .try_get::<Option<String>, _>("at_signing_key_pem")?
            .ok_or(AtpCommitError::ActorConfig("at_signing_key_pem が未設定"))?;
        let prev_commit_cid_str: Option<String> =
            actor_row.try_get::<Option<String>, _>("at_repo_cid")?;
        let prev_rev: Option<String> = actor_row.try_get::<Option<String>, _>("at_repo_rev")?;

        // ② 署名鍵をロード
        let signing_key = signing_key_from_pem(&signing_key_pem)?;

        // ③ 既存エントリをロードして新規レコードを追加・ソート
        // 同一キー（例: app.bsky.actor.profile/self の再コミット）が既に存在する場合は
        // 古いエントリを取り除いてから積む。取り除かずに push すると MST に同一キーが
        // 2つ入ってしまい、AppView がリジェクトする不正な木になる。
        let mut entries = self.load_atp_entries(actor_id).await?;
        let entry_key = format!("{}/{}", record.collection, record.rkey);
        entries.retain(|(k, _)| k != &entry_key);
        entries.push((entry_key.clone(), record.cid));
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));

        // ④ MST 構築
        let (mst_root, mst_blocks) = build_mst(&entries)?;

        // ⑤ commit 生成・P-256 署名
        let new_rev = generate_tid();
        let prev_cid_parsed = prev_commit_cid_str
            .as_deref()
            .and_then(|s| cid_from_str(s).ok());
        let (commit_cid, commit_cbor) = create_commit(
            &at_did,
            &new_rev,
            mst_root,
            prev_cid_parsed,
            &signing_key,
        )?;

        // ⑥ CAR エンコード
        let mut new_blocks = mst_blocks;
        new_blocks.push((record.cid, record.cbor));
        new_blocks.push((commit_cid, commit_cbor));
        let diff_car = encode_car(&commit_cid, &new_blocks)?;

        let commit_cid_str = cid_to_string(&commit_cid);
        let record_cid_str = cid_to_string(&record.cid);

        let mut tx = self.pool.begin().await?;

        // ⑦ atp_blocks INSERT
        for (cid, bytes) in &new_blocks {
            sqlx::query(
                "INSERT INTO atp_blocks (cid, actor_id, bytes) VALUES ($1, $2, $3)
                 ON CONFLICT (cid, actor_id) DO NOTHING",
            )
            .bind(cid_to_string(cid))
            .bind(actor_id)
            .bind(bytes.as_slice())
            .execute(&mut *tx)
            .await?;
        }

        // ⑧ actors UPDATE
        sqlx::query("UPDATE actors SET at_repo_cid = $1, at_repo_rev = $2 WHERE id = $3")
            .bind(&commit_cid_str)
            .bind(&new_rev)
            .bind(actor_id)
            .execute(&mut *tx)
            .await?;

        // ⑨ atp_records INSERT（投稿は posts テーブルで管理するためスキップ）
        // app.bsky.feed.post を atp_records にも入れると load_atp_entries で
        // posts テーブルとの二重取得になり MST に重複キーが生じて AppView に拒否される。
        if record.collection != "app.bsky.feed.post" {
            sqlx::query(
                "INSERT INTO atp_records (actor_id, collection, rkey, cid) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (actor_id, collection, rkey) DO UPDATE SET cid = EXCLUDED.cid",
            )
            .bind(actor_id)
            .bind(record.collection)
            .bind(&record.rkey)
            .bind(&record_cid_str)
            .execute(&mut *tx)
            .await?;
        }

        // ⑨.5 posts テーブル更新（commit_post 専用: post_id が指定された場合のみ）
        if let Some(pid) = post_id {
            let at_uri = format!("at://{}/app.bsky.feed.post/{}", at_did, record.rkey);
            sqlx::query(
                "UPDATE posts SET at_uri = $1, at_cid = $2, at_rkey = $3 WHERE id = $4",
            )
            .bind(&at_uri)
            .bind(&record_cid_str)
            .bind(&record.rkey)
            .bind(pid)
            .execute(&mut *tx)
            .await?;
        }

        // ⑩ atp_repo_events INSERT → seq 取得
        let ops_json = serde_json::json!([{
            "action": record.action,
            "path": entry_key,
            "cid": record_cid_str,
        }]);
        let event_row = sqlx::query(
            "INSERT INTO atp_repo_events
             (actor_id, did, commit_cid, prev_cid, rev, since_rev, car_bytes, ops_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id",
        )
        .bind(actor_id)
        .bind(&at_did)
        .bind(&commit_cid_str)
        .bind(prev_commit_cid_str.as_deref())
        .bind(&new_rev)
        .bind(prev_rev.as_deref())
        .bind(diff_car.as_slice())
        .bind(&ops_json)
        .fetch_one(&mut *tx)
        .await?;
        let seq: i64 = event_row.try_get("id")?;

        // フレームを生成して zstd 圧縮し、同一 tx 内で frame_bytes を保存する。
        // tx.commit() 前に行うことで、フレームバイト列と他のレコードの atomicity を保つ。
        let time_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let ws_ops = vec![CommitEvtOp {
            action: record.action.to_string(),
            path: entry_key,
            cid: Some(record.cid),
        }];
        let frame_opt = build_commit_frame(
            seq,
            &at_did,
            &commit_cid,
            prev_cid_parsed.as_ref(),
            &new_rev,
            prev_rev.as_deref(),
            &diff_car,
            &ws_ops,
            &record.blob_cids,
            &time_str,
        ).ok();
        if let Some(ref frame) = frame_opt {
            if let Ok(compressed) = zstd::encode_all(&frame[..], 3) {
                sqlx::query(
                    "UPDATE atp_repo_events SET frame_bytes = $1 WHERE id = $2",
                )
                .bind(&compressed)
                .bind(seq)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;

        // WebSocket ブロードキャスト
        if let Some(frame) = frame_opt {
            self.publish_event(AtpCommitEvent { frame_bytes: frame, seq });
        }

        Ok(CommitResult { commit_cid, rev: new_rev, seq, at_did })
    }

    /// ポスト作成コミット（posts テーブル更新を追加）
    ///
    /// `reply` が Some の場合は ATP `app.bsky.feed.post` の `reply` フィールドを設定する（リプライ投稿）。
    #[allow(clippy::too_many_arguments)]
    pub async fn commit_post(
        &self,
        actor_id: i64,
        post_id: i64,
        text: &str,
        facets: Vec<BskyFacet>,
        attachment_ids: &[i64],
        now: DateTime<Utc>,
        reply: Option<BskyPostReply>,
    ) -> Result<(), AtpCommitError> {
        let rkey = generate_tid();
        let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        // 添付ファイル情報を DB から取得して画像/動画/その他に分類する。
        // 動画は bsky_video_status='ready'（Bsky公式動画パイプライン結合済み）の
        // 場合のみ app.bsky.embed.video を使う。それ以外（音声・未完了の動画）は
        // 外部リンクカード（app.bsky.embed.external）にフォールバックする。
        let (bsky_images, video_candidate, non_image_url) = if !attachment_ids.is_empty() {
            let rows = sqlx::query(
                "SELECT mf.sha256, mf.size, mf.mime_type, mf.width, mf.height, mf.storage_key, sp.public_url,
                        mf.bsky_video_cid, mf.bsky_video_status
                 FROM media_files mf
                 JOIN storage_providers sp ON sp.id = mf.storage_provider_id
                 WHERE mf.id = ANY($1)
                 ORDER BY array_position($1, mf.id)",
            )
            .bind(attachment_ids)
            .fetch_all(&self.pool)
            .await?;

            let mut images: Vec<BskyImage> = Vec::new();
            let mut video_candidate: Option<BskyEmbed> = None;
            let mut non_image_url: Option<String> = None;
            for r in &rows {
                use sqlx::Row;
                let Ok(sha256) = r.try_get::<String, _>("sha256") else { continue };
                let Ok(mime_type) = r.try_get::<String, _>("mime_type") else { continue };
                let size: i64 = r.try_get("size").unwrap_or(0);
                let width: Option<i32> = r.try_get("width").unwrap_or(None);
                let height: Option<i32> = r.try_get("height").unwrap_or(None);
                if mime_type.starts_with("image/") {
                    // CID 生成に失敗したものはスキップ
                    if cid_from_sha256_hex(&sha256).is_err() { continue; }
                    images.push(BskyImage {
                        sha256_hex: sha256, mime_type, size,
                        width: width.unwrap_or(0), height: height.unwrap_or(0),
                        alt: String::new(),
                    });
                    continue;
                }
                if mime_type.starts_with("video/") && video_candidate.is_none() {
                    let status: Option<String> = r.try_get("bsky_video_status").unwrap_or(None);
                    let video_cid: Option<String> = r.try_get("bsky_video_cid").unwrap_or(None);
                    if status.as_deref() == Some("ready") {
                        if let Some(video_cid) = video_cid {
                            video_candidate = Some(BskyEmbed::Video {
                                cid: video_cid, mime_type: mime_type.clone(), size,
                                width: width.unwrap_or(0), height: height.unwrap_or(0),
                            });
                            continue;
                        }
                    }
                }
                if non_image_url.is_none() {
                    if let (Ok(storage_key), Ok(public_url)) = (r.try_get::<String, _>("storage_key"), r.try_get::<String, _>("public_url")) {
                        non_image_url = Some(format!("{}/{}", public_url.trim_end_matches('/'), storage_key));
                    }
                }
            }
            (images, video_candidate, non_image_url)
        } else {
            (vec![], None, None)
        };

        // app.bsky.embed.images の上限は 4 枚（AT Protocol 仕様）。
        // ポスト自体は最大 10 枚まで許容するが、Bsky embed には先頭 4 枚のみ含める。
        let bsky_images: Vec<BskyImage> = bsky_images.into_iter().take(4).collect();

        let mut blob_cids: Vec<Cid> = bsky_images.iter()
            .filter_map(|img| cid_from_sha256_hex(&img.sha256_hex).ok())
            .collect();

        let embed = if !bsky_images.is_empty() {
            Some(BskyEmbed::Images(bsky_images))
        } else if let Some(video_embed) = video_candidate {
            if let BskyEmbed::Video { ref cid, .. } = video_embed {
                if let Ok(video_cid) = cid_from_str(cid) {
                    blob_cids.push(video_cid);
                }
            }
            Some(video_embed)
        } else {
            non_image_url.map(|url| BskyEmbed::External { url })
        };
        let (record_cbor, record_cid) = encode_bsky_feed_post(text, &created_at_str, facets, embed, reply)?;
        let record_cid_str = cid_to_string(&record_cid);

        let record = CommitRecord {
            collection: "app.bsky.feed.post",
            rkey: rkey.clone(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
            blob_cids,
        };

        let result = self.commit_record_inner(actor_id, record, now, Some(post_id)).await?;

        let at_uri = format!("at://{}/app.bsky.feed.post/{}", result.at_did, rkey);
        tracing::info!("[atp] commit 完了: at_uri={}, cid={}", at_uri, record_cid_str);
        self.spawn_request_crawl();
        Ok(())
    }

    /// Bsky リポストコミット（`app.bsky.feed.repost` レコードを ATP リポジトリにコミット）。
    ///
    /// `at_uri` / `at_cid` は元ポストの ATP URI と CID。
    /// posts テーブルを更新しない（リポストは atp_records で管理）。
    pub async fn commit_repost(
        &self,
        actor_id: i64,
        at_uri: &str,
        at_cid: &str,
        now: DateTime<Utc>,
        post_id: Option<i64>,
    ) -> Result<(), AtpCommitError> {
        let rkey = generate_tid();
        let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let (record_cbor, record_cid) = encode_bsky_feed_repost(at_uri, at_cid, &created_at_str)?;

        let record = CommitRecord {
            collection: "app.bsky.feed.repost",
            rkey: rkey.clone(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
            blob_cids: vec![],
        };

        self.commit_record_inner(actor_id, record, now, None).await?;

        if let Some(pid) = post_id {
            sqlx::query("UPDATE posts SET atp_repost_rkey = $1 WHERE id = $2")
                .bind(&rkey)
                .bind(pid)
                .execute(&self.pool)
                .await?;
        }

        tracing::info!("[atp] repost commit 完了: actor_id={}, rkey={}", actor_id, rkey);
        self.spawn_request_crawl();
        Ok(())
    }

    /// `app.bsky.feed.like` レコードをコミットする（リアクション連携）。
    /// ATP には絵文字リアクションの概念が無いため、どの絵文字でも Like として送る。
    /// `emoji` は非標準の拡張フィールドとしてベストエフォートで載せる。
    /// 成功したら生成した at_uri（`at://did/app.bsky.feed.like/rkey`）を
    /// `reactions.at_uri` に自己保存する（`commit_repost` が `posts.atp_repost_rkey` を
    /// 自己保存するのと同じ流儀）。切替（別の絵文字への変更）の場合、旧 Like の削除は
    /// 呼び出し側が事前に `delete_atp_like` で行う（このメソッドは新規作成のみを担う）。
    pub async fn commit_like(
        &self,
        actor_id: i64,
        post_id: i64,
        target_at_uri: &str,
        target_at_cid: &str,
        emoji: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        let rkey = generate_tid();
        let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let (record_cbor, record_cid) = encode_bsky_feed_like(target_at_uri, target_at_cid, &created_at_str, emoji)?;

        let record = CommitRecord {
            collection: "app.bsky.feed.like",
            rkey: rkey.clone(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
            blob_cids: vec![],
        };

        let result = self.commit_record_inner(actor_id, record, now, None).await?;

        let at_uri_self = format!("at://{}/app.bsky.feed.like/{}", result.at_did, rkey);
        sqlx::query("UPDATE reactions SET at_uri = $1 WHERE post_id = $2 AND actor_id = $3")
            .bind(&at_uri_self)
            .bind(post_id)
            .bind(actor_id)
            .execute(&self.pool)
            .await?;

        tracing::info!("[atp] like commit 完了: actor_id={}, rkey={}", actor_id, rkey);
        self.spawn_request_crawl();
        Ok(())
    }

    /// `app.bsky.graph.follow` レコードをコミットする。
    /// 成功時は生成した rkey を返す（将来のアンフォロー時に必要）。
    pub async fn commit_follow(
        &self,
        actor_id: i64,
        subject_did: &str,
        now: DateTime<Utc>,
    ) -> Result<String, AtpCommitError> {
        let rkey = generate_tid();
        let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let (record_cbor, record_cid) = encode_bsky_graph_follow(subject_did, &created_at_str)?;

        let record = CommitRecord {
            collection: "app.bsky.graph.follow",
            rkey: rkey.clone(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
            blob_cids: vec![],
        };

        self.commit_record_inner(actor_id, record, now, None).await?;

        tracing::info!("[atp] follow commit 完了: actor_id={}, subject={}, rkey={}", actor_id, subject_did, rkey);
        self.spawn_request_crawl();
        Ok(rkey)
    }

    /// リポスト解除コミット（app.bsky.feed.repost レコードを MST から削除する）。
    pub async fn delete_atp_repost(
        &self,
        actor_id: i64,
        rkey: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        // ① アクター情報取得
        let actor_row = sqlx::query(
            "SELECT at_did, at_signing_key_pem, at_repo_cid, at_repo_rev
             FROM actors WHERE id = $1",
        )
        .bind(actor_id)
        .fetch_one(&self.pool)
        .await?;

        let at_did: String = actor_row
            .try_get::<Option<String>, _>("at_did")?
            .ok_or(AtpCommitError::ActorConfig("at_did が未設定"))?;
        let signing_key_pem: String = actor_row
            .try_get::<Option<String>, _>("at_signing_key_pem")?
            .ok_or(AtpCommitError::ActorConfig("at_signing_key_pem が未設定"))?;
        let prev_commit_cid_str: Option<String> =
            actor_row.try_get::<Option<String>, _>("at_repo_cid")?;
        let prev_rev: Option<String> = actor_row.try_get::<Option<String>, _>("at_repo_rev")?;

        // ② 署名鍵をロード
        let signing_key = signing_key_from_pem(&signing_key_pem)?;

        // ③ 既存エントリをロードして対象レコードを除去
        let mut entries = self.load_atp_entries(actor_id).await?;
        let entry_key = format!("app.bsky.feed.repost/{}", rkey);
        entries.retain(|(k, _)| k != &entry_key);
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));

        // ④ MST 構築
        let (mst_root, mst_blocks) = build_mst(&entries)?;

        // ⑤ commit 生成・P-256 署名
        let new_rev = generate_tid();
        let prev_cid_parsed = prev_commit_cid_str
            .as_deref()
            .and_then(|s| cid_from_str(s).ok());
        let (commit_cid, commit_cbor) = create_commit(
            &at_did,
            &new_rev,
            mst_root,
            prev_cid_parsed,
            &signing_key,
        )?;

        // ⑥ CAR エンコード（削除レコードのブロックは含まない）
        let mut new_blocks = mst_blocks;
        new_blocks.push((commit_cid, commit_cbor));
        let diff_car = encode_car(&commit_cid, &new_blocks)?;

        let commit_cid_str = cid_to_string(&commit_cid);

        let mut tx = self.pool.begin().await?;

        // ⑦ atp_blocks INSERT
        for (cid, bytes) in &new_blocks {
            sqlx::query(
                "INSERT INTO atp_blocks (cid, actor_id, bytes) VALUES ($1, $2, $3)
                 ON CONFLICT (cid, actor_id) DO NOTHING",
            )
            .bind(cid_to_string(cid))
            .bind(actor_id)
            .bind(bytes.as_slice())
            .execute(&mut *tx)
            .await?;
        }

        // ⑧ actors UPDATE
        sqlx::query("UPDATE actors SET at_repo_cid = $1, at_repo_rev = $2 WHERE id = $3")
            .bind(&commit_cid_str)
            .bind(&new_rev)
            .bind(actor_id)
            .execute(&mut *tx)
            .await?;

        // ⑨ atp_records DELETE
        sqlx::query(
            "DELETE FROM atp_records WHERE actor_id = $1 AND collection = 'app.bsky.feed.repost' AND rkey = $2",
        )
        .bind(actor_id)
        .bind(rkey)
        .execute(&mut *tx)
        .await?;

        // ⑩ atp_repo_events INSERT
        let ops_json = serde_json::json!([{
            "action": "delete",
            "path": entry_key,
        }]);
        let event_row = sqlx::query(
            "INSERT INTO atp_repo_events
             (actor_id, did, commit_cid, prev_cid, rev, since_rev, car_bytes, ops_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id",
        )
        .bind(actor_id)
        .bind(&at_did)
        .bind(&commit_cid_str)
        .bind(prev_commit_cid_str.as_deref())
        .bind(&new_rev)
        .bind(prev_rev.as_deref())
        .bind(diff_car.as_slice())
        .bind(&ops_json)
        .fetch_one(&mut *tx)
        .await?;
        let seq: i64 = event_row.try_get("id")?;

        let time_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let ws_ops = vec![CommitEvtOp {
            action: "delete".to_string(),
            path: entry_key,
            cid: None,
        }];
        let frame_opt = build_commit_frame(
            seq,
            &at_did,
            &commit_cid,
            prev_cid_parsed.as_ref(),
            &new_rev,
            prev_rev.as_deref(),
            &diff_car,
            &ws_ops,
            &[],
            &time_str,
        ).ok();
        if let Some(ref frame) = frame_opt {
            if let Ok(compressed) = zstd::encode_all(&frame[..], 3) {
                sqlx::query(
                    "UPDATE atp_repo_events SET frame_bytes = $1 WHERE id = $2",
                )
                .bind(&compressed)
                .bind(seq)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;

        if let Some(frame) = frame_opt {
            self.publish_event(AtpCommitEvent { frame_bytes: frame, seq });
        }

        tracing::info!("[atp] repost delete commit 完了: actor_id={}, rkey={}", actor_id, rkey);
        self.spawn_request_crawl();
        Ok(())
    }

    /// リアクション取消/切替コミット（`app.bsky.feed.like` レコードを MST から削除する）。
    /// `reactions.at_uri` のクリアは呼び出し側の責務
    /// （ローカル取消は行自体を DELETE するので不要。切替は直後の `commit_like` が上書きする）。
    pub async fn delete_atp_like(
        &self,
        actor_id: i64,
        rkey: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        // ① アクター情報取得
        let actor_row = sqlx::query(
            "SELECT at_did, at_signing_key_pem, at_repo_cid, at_repo_rev
             FROM actors WHERE id = $1",
        )
        .bind(actor_id)
        .fetch_one(&self.pool)
        .await?;

        let at_did: String = actor_row
            .try_get::<Option<String>, _>("at_did")?
            .ok_or(AtpCommitError::ActorConfig("at_did が未設定"))?;
        let signing_key_pem: String = actor_row
            .try_get::<Option<String>, _>("at_signing_key_pem")?
            .ok_or(AtpCommitError::ActorConfig("at_signing_key_pem が未設定"))?;
        let prev_commit_cid_str: Option<String> =
            actor_row.try_get::<Option<String>, _>("at_repo_cid")?;
        let prev_rev: Option<String> = actor_row.try_get::<Option<String>, _>("at_repo_rev")?;

        // ② 署名鍵をロード
        let signing_key = signing_key_from_pem(&signing_key_pem)?;

        // ③ 既存エントリをロードして対象レコードを除去
        let mut entries = self.load_atp_entries(actor_id).await?;
        let entry_key = format!("app.bsky.feed.like/{}", rkey);
        entries.retain(|(k, _)| k != &entry_key);
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));

        // ④ MST 構築
        let (mst_root, mst_blocks) = build_mst(&entries)?;

        // ⑤ commit 生成・P-256 署名
        let new_rev = generate_tid();
        let prev_cid_parsed = prev_commit_cid_str
            .as_deref()
            .and_then(|s| cid_from_str(s).ok());
        let (commit_cid, commit_cbor) = create_commit(
            &at_did,
            &new_rev,
            mst_root,
            prev_cid_parsed,
            &signing_key,
        )?;

        // ⑥ CAR エンコード（削除レコードのブロックは含まない）
        let mut new_blocks = mst_blocks;
        new_blocks.push((commit_cid, commit_cbor));
        let diff_car = encode_car(&commit_cid, &new_blocks)?;

        let commit_cid_str = cid_to_string(&commit_cid);

        let mut tx = self.pool.begin().await?;

        // ⑦ atp_blocks INSERT
        for (cid, bytes) in &new_blocks {
            sqlx::query(
                "INSERT INTO atp_blocks (cid, actor_id, bytes) VALUES ($1, $2, $3)
                 ON CONFLICT (cid, actor_id) DO NOTHING",
            )
            .bind(cid_to_string(cid))
            .bind(actor_id)
            .bind(bytes.as_slice())
            .execute(&mut *tx)
            .await?;
        }

        // ⑧ actors UPDATE
        sqlx::query("UPDATE actors SET at_repo_cid = $1, at_repo_rev = $2 WHERE id = $3")
            .bind(&commit_cid_str)
            .bind(&new_rev)
            .bind(actor_id)
            .execute(&mut *tx)
            .await?;

        // ⑨ atp_records DELETE
        sqlx::query(
            "DELETE FROM atp_records WHERE actor_id = $1 AND collection = 'app.bsky.feed.like' AND rkey = $2",
        )
        .bind(actor_id)
        .bind(rkey)
        .execute(&mut *tx)
        .await?;

        // ⑩ atp_repo_events INSERT
        let ops_json = serde_json::json!([{
            "action": "delete",
            "path": entry_key,
        }]);
        let event_row = sqlx::query(
            "INSERT INTO atp_repo_events
             (actor_id, did, commit_cid, prev_cid, rev, since_rev, car_bytes, ops_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id",
        )
        .bind(actor_id)
        .bind(&at_did)
        .bind(&commit_cid_str)
        .bind(prev_commit_cid_str.as_deref())
        .bind(&new_rev)
        .bind(prev_rev.as_deref())
        .bind(diff_car.as_slice())
        .bind(&ops_json)
        .fetch_one(&mut *tx)
        .await?;
        let seq: i64 = event_row.try_get("id")?;

        let time_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let ws_ops = vec![CommitEvtOp {
            action: "delete".to_string(),
            path: entry_key,
            cid: None,
        }];
        let frame_opt = build_commit_frame(
            seq,
            &at_did,
            &commit_cid,
            prev_cid_parsed.as_ref(),
            &new_rev,
            prev_rev.as_deref(),
            &diff_car,
            &ws_ops,
            &[],
            &time_str,
        ).ok();
        if let Some(ref frame) = frame_opt {
            if let Ok(compressed) = zstd::encode_all(&frame[..], 3) {
                sqlx::query(
                    "UPDATE atp_repo_events SET frame_bytes = $1 WHERE id = $2",
                )
                .bind(&compressed)
                .bind(seq)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;

        if let Some(frame) = frame_opt {
            self.publish_event(AtpCommitEvent { frame_bytes: frame, seq });
        }

        tracing::info!("[atp] like delete commit 完了: actor_id={}, rkey={}", actor_id, rkey);
        self.spawn_request_crawl();
        Ok(())
    }

    /// `app.bsky.graph.follow` レコードを削除コミットする（アンフォロー）。
    pub async fn commit_delete_follow(
        &self,
        actor_id: i64,
        rkey: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        let actor_row = sqlx::query(
            "SELECT at_did, at_signing_key_pem, at_repo_cid, at_repo_rev
             FROM actors WHERE id = $1",
        )
        .bind(actor_id)
        .fetch_one(&self.pool)
        .await?;

        let at_did: String = actor_row
            .try_get::<Option<String>, _>("at_did")?
            .ok_or(AtpCommitError::ActorConfig("at_did が未設定"))?;
        let signing_key_pem: String = actor_row
            .try_get::<Option<String>, _>("at_signing_key_pem")?
            .ok_or(AtpCommitError::ActorConfig("at_signing_key_pem が未設定"))?;
        let prev_commit_cid_str: Option<String> =
            actor_row.try_get::<Option<String>, _>("at_repo_cid")?;
        let prev_rev: Option<String> = actor_row.try_get::<Option<String>, _>("at_repo_rev")?;

        let signing_key = signing_key_from_pem(&signing_key_pem)?;

        let mut entries = self.load_atp_entries(actor_id).await?;
        let entry_key = format!("app.bsky.graph.follow/{}", rkey);
        entries.retain(|(k, _)| k != &entry_key);
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));

        let (mst_root, mst_blocks) = build_mst(&entries)?;

        let new_rev = generate_tid();
        let prev_cid_parsed = prev_commit_cid_str
            .as_deref()
            .and_then(|s| cid_from_str(s).ok());
        let (commit_cid, commit_cbor) = create_commit(
            &at_did, &new_rev, mst_root, prev_cid_parsed, &signing_key,
        )?;

        let mut new_blocks = mst_blocks;
        new_blocks.push((commit_cid, commit_cbor));
        let diff_car = encode_car(&commit_cid, &new_blocks)?;
        let commit_cid_str = cid_to_string(&commit_cid);

        let mut tx = self.pool.begin().await?;

        for (cid, bytes) in &new_blocks {
            sqlx::query(
                "INSERT INTO atp_blocks (cid, actor_id, bytes) VALUES ($1, $2, $3)
                 ON CONFLICT (cid, actor_id) DO NOTHING",
            )
            .bind(cid_to_string(cid))
            .bind(actor_id)
            .bind(bytes.as_slice())
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query("UPDATE actors SET at_repo_cid = $1, at_repo_rev = $2 WHERE id = $3")
            .bind(&commit_cid_str)
            .bind(&new_rev)
            .bind(actor_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            "DELETE FROM atp_records WHERE actor_id = $1 AND collection = 'app.bsky.graph.follow' AND rkey = $2",
        )
        .bind(actor_id)
        .bind(rkey)
        .execute(&mut *tx)
        .await?;

        let ops_json = serde_json::json!([{"action": "delete", "path": entry_key}]);
        let event_row = sqlx::query(
            "INSERT INTO atp_repo_events
             (actor_id, did, commit_cid, prev_cid, rev, since_rev, car_bytes, ops_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id",
        )
        .bind(actor_id)
        .bind(&at_did)
        .bind(&commit_cid_str)
        .bind(prev_commit_cid_str.as_deref())
        .bind(&new_rev)
        .bind(prev_rev.as_deref())
        .bind(diff_car.as_slice())
        .bind(&ops_json)
        .fetch_one(&mut *tx)
        .await?;
        let seq: i64 = event_row.try_get("id")?;

        let time_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let ws_ops = vec![CommitEvtOp { action: "delete".to_string(), path: entry_key, cid: None }];
        let frame_opt = build_commit_frame(
            seq, &at_did, &commit_cid, prev_cid_parsed.as_ref(),
            &new_rev, prev_rev.as_deref(), &diff_car, &ws_ops, &[], &time_str,
        ).ok();
        if let Some(ref frame) = frame_opt {
            if let Ok(compressed) = zstd::encode_all(&frame[..], 3) {
                sqlx::query("UPDATE atp_repo_events SET frame_bytes = $1 WHERE id = $2")
                    .bind(&compressed)
                    .bind(seq)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        tx.commit().await?;

        if let Some(frame) = frame_opt {
            self.publish_event(AtpCommitEvent { frame_bytes: frame, seq });
        }

        tracing::info!("[atp] follow delete commit 完了: actor_id={}, rkey={}", actor_id, rkey);
        self.spawn_request_crawl();
        Ok(())
    }

    /// Bsky テキスト投稿コミット（DB の posts レコードを更新しない）。
    ///
    /// リポストの Bsky フォールバック投稿など、DB にポストレコードを作らずに
    /// ATP リポジトリにテキストポストだけ送信したい場合に使用する。
    pub async fn commit_standalone_text_post(
        &self,
        actor_id: i64,
        text: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        let rkey = generate_tid();
        let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let (record_cbor, record_cid) = encode_bsky_feed_post(text, &created_at_str, vec![], None, None)?;

        let record = CommitRecord {
            collection: "app.bsky.feed.post",
            rkey: rkey.clone(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
            blob_cids: vec![],
        };

        let result = self.commit_record_inner(actor_id, record, now, None).await?;

        tracing::info!("[atp] standalone text post commit 完了: did={}, rkey={}", result.at_did, rkey);
        self.spawn_request_crawl();
        Ok(())
    }

    /// 引用投稿コミット（`app.bsky.embed.record` または `app.bsky.embed.external` 付き）。
    ///
    /// `embed` に `BskyEmbed::Record` を渡すと Bsky ネイティブ引用、
    /// `BskyEmbed::External` を渡すと URL カードとして送信する。
    /// DB の posts レコードは呼び出し元で更新済みである前提。
    #[allow(clippy::too_many_arguments)]
    pub async fn commit_quote(
        &self,
        actor_id: i64,
        post_id: i64,
        text: &str,
        facets: Vec<BskyFacet>,
        embed: Option<BskyEmbed>,
        now: DateTime<Utc>,
        reply: Option<BskyPostReply>,
    ) -> Result<(), AtpCommitError> {
        let rkey = generate_tid();
        let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let (record_cbor, record_cid) = encode_bsky_feed_post(text, &created_at_str, facets, embed, reply)?;
        let record_cid_str = cid_to_string(&record_cid);

        let record = CommitRecord {
            collection: "app.bsky.feed.post",
            rkey: rkey.clone(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
            blob_cids: vec![],
        };

        let result = self.commit_record_inner(actor_id, record, now, Some(post_id)).await?;

        let at_uri = format!("at://{}/app.bsky.feed.post/{}", result.at_did, rkey);
        tracing::info!("[atp] quote commit 完了: at_uri={}, cid={}", at_uri, record_cid_str);
        self.spawn_request_crawl();
        Ok(())
    }

    /// プロフィール（再）コミット。新規登録時は avatar/description なしで呼ばれ、
    /// プロフィール編集時は bio・アバター blob 情報を渡して再コミットする。
    /// 既に `app.bsky.actor.profile/self` が存在するかで action(create/update)を自動判定する。
    pub async fn commit_profile(
        &self,
        actor_id: i64,
        display_name: &str,
        description: Option<&str>,
        avatar_media: Option<(String, String, i64)>,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        let existing: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM atp_records \
             WHERE actor_id = $1 AND collection = 'app.bsky.actor.profile' AND rkey = 'self')",
        )
        .bind(actor_id)
        .fetch_one(&self.pool)
        .await?;
        let action: &'static str = if existing { "update" } else { "create" };

        let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let avatar_ref = avatar_media
            .as_ref()
            .map(|(sha256, mime, size)| (sha256.as_str(), mime.as_str(), *size));
        let (record_cbor, record_cid) =
            encode_bsky_actor_profile(display_name, description, avatar_ref, &created_at_str)?;

        let record = CommitRecord {
            collection: "app.bsky.actor.profile",
            rkey: "self".to_string(),
            cbor: record_cbor,
            cid: record_cid,
            action,
            blob_cids: vec![],
        };

        let result = self.commit_record_inner(actor_id, record, now, None).await?;

        tracing::info!("[atp] profile commit 完了（{}）: did={}", action, result.at_did);
        self.spawn_request_crawl();
        Ok(())
    }

    /// #identity イベントを DB に保存して subscribeRepos にブロードキャストする。
    /// ユーザー登録完了後に呼び出し、Relay/AppView に handle の再検証を促す。
    pub async fn broadcast_identity_event(
        &self,
        actor_id: i64,
        did: &str,
        handle: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        let time_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        // まず seq を確保するために frame_bytes なしで INSERT する。
        let seq: i64 = sqlx::query_scalar(
            "INSERT INTO atp_repo_events (event_type, actor_id, did, handle)
             VALUES ('identity', $1, $2, $3)
             RETURNING id",
        )
        .bind(actor_id)
        .bind(did)
        .bind(handle)
        .fetch_one(&self.pool)
        .await?;

        // 実 seq でフレームを生成し、圧縮して DB に保存してからブロードキャスト。
        let frame = build_identity_frame(seq, did, handle, &time_str)?;
        let compressed = zstd::encode_all(&frame[..], 3)
            .map_err(|e| RepoError::Cbor(e.to_string()))?;
        sqlx::query("UPDATE atp_repo_events SET frame_bytes = $1 WHERE id = $2")
            .bind(&compressed)
            .bind(seq)
            .execute(&self.pool)
            .await?;

        self.publish_event(AtpCommitEvent { frame_bytes: frame, seq });

        tracing::info!("[atp] identity broadcast: did={}, handle={}, seq={}", did, handle, seq);
        Ok(())
    }

    /// #account イベントを DB に保存して subscribeRepos にブロードキャストする。
    /// `active=false, status="deleted"` でアカウント削除を AppView/Relay に通知する（退会機能 #29）。
    pub async fn broadcast_account_event(
        &self,
        actor_id: i64,
        did: &str,
        handle: &str,
        now: DateTime<Utc>,
        active: bool,
        status: Option<&str>,
    ) -> Result<(), AtpCommitError> {
        let time_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let seq: i64 = sqlx::query_scalar(
            "INSERT INTO atp_repo_events (event_type, actor_id, did, handle)
             VALUES ('account', $1, $2, $3)
             RETURNING id",
        )
        .bind(actor_id)
        .bind(did)
        .bind(handle)
        .fetch_one(&self.pool)
        .await?;

        let frame = build_account_frame(seq, did, handle, &time_str, active, status)?;
        let compressed = zstd::encode_all(&frame[..], 3)
            .map_err(|e| RepoError::Cbor(e.to_string()))?;
        sqlx::query("UPDATE atp_repo_events SET frame_bytes = $1 WHERE id = $2")
            .bind(&compressed)
            .bind(seq)
            .execute(&self.pool)
            .await?;

        self.publish_event(AtpCommitEvent { frame_bytes: frame, seq });

        tracing::info!("[atp] account broadcast: did={}, active={}, seq={}", did, active, seq);
        Ok(())
    }
}

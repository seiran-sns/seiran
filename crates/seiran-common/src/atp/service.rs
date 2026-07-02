use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::atp::plc::{signing_key_from_pem, PlcError};
use crate::atp::repo::{
    build_commit_frame, build_mst, cid_from_str, cid_to_string, create_commit, encode_car,
    encode_bsky_actor_profile, encode_bsky_feed_post, generate_tid, Cid, CommitEvtOp, RepoError,
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
#[derive(Clone)]
pub struct AtpCommitEvent {
    pub frame_bytes: Vec<u8>,
    #[allow(dead_code)]
    pub seq: i64,
}

/// コミット処理に渡すレコード情報
pub struct CommitRecord {
    pub collection: &'static str,
    pub rkey: String,
    pub cbor: Vec<u8>,
    pub cid: Cid,
    pub action: &'static str,
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
}

impl AtpCommitService {
    pub fn new(
        pool: PgPool,
        event_tx: Arc<broadcast::Sender<AtpCommitEvent>>,
        http_client: Arc<reqwest::Client>,
    ) -> Self {
        Self { pool, event_tx, http_client }
    }

    pub fn event_tx(&self) -> &Arc<broadcast::Sender<AtpCommitEvent>> {
        &self.event_tx
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
                    Ok(res) => eprintln!("[atp] requestCrawl → {}", res.status()),
                    Err(e) => eprintln!("[atp] requestCrawl 失敗: {}", e),
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
        let mut entries = self.load_atp_entries(actor_id).await?;
        let entry_key = format!("{}/{}", record.collection, record.rkey);
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

        tx.commit().await?;

        // WebSocket ブロードキャスト
        let time_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let ws_ops = vec![CommitEvtOp {
            action: record.action.to_string(),
            path: entry_key,
            cid: record.cid,
        }];
        if let Ok(frame) = build_commit_frame(
            seq,
            &at_did,
            &commit_cid,
            prev_cid_parsed.as_ref(),
            &new_rev,
            prev_rev.as_deref(),
            &diff_car,
            &ws_ops,
            &time_str,
        ) {
            let _ = self.event_tx.send(AtpCommitEvent { frame_bytes: frame, seq });
        }

        Ok(CommitResult { commit_cid, rev: new_rev, seq, at_did })
    }

    /// ポスト作成コミット（posts テーブル更新を追加）
    pub async fn commit_post(
        &self,
        actor_id: i64,
        post_id: i64,
        text: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        let rkey = generate_tid();
        let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let (record_cbor, record_cid) = encode_bsky_feed_post(text, &created_at_str)?;
        let record_cid_str = cid_to_string(&record_cid);

        let record = CommitRecord {
            collection: "app.bsky.feed.post",
            rkey: rkey.clone(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
        };

        let result = self.commit_record_inner(actor_id, record, now, Some(post_id)).await?;

        let at_uri = format!("at://{}/app.bsky.feed.post/{}", result.at_did, rkey);
        eprintln!("[atp] commit 完了: at_uri={}, cid={}", at_uri, record_cid_str);
        self.spawn_request_crawl();
        Ok(())
    }

    /// プロフィール登録コミット（初回コミット後に requestCrawl）
    pub async fn commit_profile(
        &self,
        actor_id: i64,
        display_name: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let (record_cbor, record_cid) = encode_bsky_actor_profile(display_name, &created_at_str)?;

        let record = CommitRecord {
            collection: "app.bsky.actor.profile",
            rkey: "self".to_string(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
        };

        let result = self.commit_record_inner(actor_id, record, now, None).await?;

        eprintln!("[atp] profile commit 完了: did={}", result.at_did);
        self.spawn_request_crawl();
        Ok(())
    }
}

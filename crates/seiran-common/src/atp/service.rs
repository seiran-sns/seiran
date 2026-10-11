use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::sync::Arc;
use std::time::Duration;

use crate::event_bus::EventBus;

use crate::atp::plc::{signing_key_from_pem, PlcError};
use crate::atp::repo::{
    build_account_frame, build_commit_frame, build_identity_frame, build_mst, cid_from_sha256_hex,
    cid_from_str, cid_to_string, collect_blob_cids, create_commit, encode_bsky_actor_profile,
    encode_bsky_feed_like, encode_bsky_feed_post, encode_bsky_feed_repost, encode_bsky_graph_block,
    encode_bsky_graph_follow, encode_bsky_graph_list, encode_bsky_graph_listitem, encode_car,
    encode_chat_actor_declaration, encode_content_visibility_declaration, encode_generic_record,
    encode_seiran_actor_declaration, generate_tid, BskyEmbed, BskyFacet, BskyPostReply, Cid,
    CommitEvtOp, RepoError,
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

/// Redis でプロセス間の配信をつなぐときのチャンネル名。
const ATP_EVENTS_CHANNEL: &str = "seiran:atp:events";
/// subscribeRepos の接続が読み遅れたときに保持しておくイベント数。
const ATP_EVENTS_CAPACITY: usize = 1024;

impl AtpCommitEvent {
    /// subscribeRepos へ配るイベントのバス。`redis` があれば全プロセスの接続へ配る
    /// （どのプロセスでコミットしても、どのプロセスにつないだリレーにも届く）。
    pub async fn bus(
        redis: Option<&redis::Client>,
    ) -> Result<EventBus<AtpCommitEvent>, redis::RedisError> {
        match redis {
            Some(client) => {
                EventBus::with_redis(ATP_EVENTS_CAPACITY, client, ATP_EVENTS_CHANNEL).await
            }
            None => Ok(EventBus::local(ATP_EVENTS_CAPACITY)),
        }
    }
}

/// コミット処理に渡すレコード情報
pub struct CommitRecord {
    pub collection: String,
    pub rkey: String,
    pub cbor: Vec<u8>,
    pub cid: Cid,
    pub action: &'static str,
    pub blob_cids: Vec<Cid>,
}

/// コミットで MST に加える変更。
enum RepoWrite {
    Put(CommitRecord),
    Delete { collection: String, rkey: String },
}

impl RepoWrite {
    fn collection(&self) -> &str {
        match self {
            RepoWrite::Put(record) => &record.collection,
            RepoWrite::Delete { collection, .. } => collection,
        }
    }

    fn rkey(&self) -> &str {
        match self {
            RepoWrite::Put(record) => &record.rkey,
            RepoWrite::Delete { rkey, .. } => rkey,
        }
    }
}

/// 直列化失敗でコミットをやり直す上限。同じアクターへのコミットが同時に数件重なる程度を想定する。
const MAX_SERIALIZATION_RETRIES: u32 = 10;

/// コミット処理の結果
pub struct CommitResult {
    pub commit_cid: Cid,
    pub rev: String,
    pub seq: i64,
    pub at_did: String,
}

/// `atp_blocks` へ新規ブロックを一括INSERTする。コミットのたびに数個〜数十個の
/// ブロックが生じるため、1件ずつ`INSERT`するとラウンドトリップがブロック数分かかる。
/// `UNNEST`で配列を展開し1回のクエリにまとめる。
async fn insert_blocks(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_id: i64,
    blocks: &[(Cid, Vec<u8>)],
) -> Result<(), sqlx::Error> {
    if blocks.is_empty() {
        return Ok(());
    }
    let cids: Vec<String> = blocks.iter().map(|(cid, _)| cid_to_string(cid)).collect();
    let actor_ids: Vec<i64> = vec![actor_id; blocks.len()];
    let bytes: Vec<Vec<u8>> = blocks.iter().map(|(_, b)| b.clone()).collect();
    sqlx::query(
        "INSERT INTO atp_blocks (cid, actor_id, bytes)
         SELECT * FROM UNNEST($1::text[], $2::bigint[], $3::bytea[])
         ON CONFLICT (cid, actor_id) DO NOTHING",
    )
    .bind(&cids)
    .bind(&actor_ids)
    .bind(&bytes)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

/// `BskyEmbed` が参照するblob（画像/動画/外部カードサムネイル）のCIDを列挙する。
/// `CommitRecord::blob_cids`（MSTコミットに含めるblobの一覧）の算出に使う。
fn blob_cids_for_embed(embed: &Option<BskyEmbed>) -> Vec<Cid> {
    match embed {
        Some(BskyEmbed::Images(images)) => images
            .iter()
            .filter_map(|img| cid_from_sha256_hex(&img.sha256_hex).ok())
            .collect(),
        Some(BskyEmbed::Video { cid, .. }) => cid_from_str(cid).ok().into_iter().collect(),
        Some(BskyEmbed::External {
            thumb: Some(thumb), ..
        }) => cid_from_sha256_hex(&thumb.sha256_hex)
            .ok()
            .into_iter()
            .collect(),
        Some(BskyEmbed::External { thumb: None, .. }) | Some(BskyEmbed::Record { .. }) | None => {
            Vec::new()
        }
        Some(BskyEmbed::RecordWithMedia { media, .. }) => blob_cids_for_media(media),
    }
}

/// `blob_cids_for_embed`の`media`（`Images`/`Video`/`External`のいずれか）専用版。
fn blob_cids_for_media(media: &BskyEmbed) -> Vec<Cid> {
    match media {
        BskyEmbed::Images(images) => images
            .iter()
            .filter_map(|img| cid_from_sha256_hex(&img.sha256_hex).ok())
            .collect(),
        BskyEmbed::Video { cid, .. } => cid_from_str(cid).ok().into_iter().collect(),
        BskyEmbed::External {
            thumb: Some(thumb), ..
        } => cid_from_sha256_hex(&thumb.sha256_hex)
            .ok()
            .into_iter()
            .collect(),
        BskyEmbed::External { thumb: None, .. }
        | BskyEmbed::Record { .. }
        | BskyEmbed::RecordWithMedia { .. } => Vec::new(),
    }
}

/// ATPコミット（`commit_post`）用の`seiranPost`拡張オブジェクトを組み立てる（#237）。
/// `counterpartPostId`（AP object id）は投稿作成時点でローカル生成済み・常に確定しているため、
/// AP側（`counterpartPostId`が未確定ならUpdate(Note)で後から補完）と異なり毎回必ず同梱できる。
/// 投稿者がまだAP側の識別子（`ap_uri`）を持たない異常系は実運用上あり得ない
/// （ローカルアクターは登録時に`insert_local`が`ap_uri`を必ず設定する）ため、
/// 取得できなければログのみでseiranPost自体を諦める（コミット自体は失敗させない）。
///
/// `posts.body`（DBの生プレーンテキスト）はドメイン省略のローカル短縮メンション（`@user`）を
/// そのまま含みうる。これを`seiranPost.body`へ生のまま埋め込むと、受信側の他seiranサーバーでは
/// 別ユーザーへのメンションと誤認される（AP側`build_seiran_post_for_basis`と同種）。このため`commit_post`が渡す`text`
/// （`convert_mentions_for_bsky`変換済み、ATPハンドル形式`@user.domain`）は使わず、AP側と表記を
/// 揃えるためここで改めて`convert_mentions_for_ap`（Fediverse形式`@user@domain`）を掛け直す。
async fn build_seiran_post_for_atp_commit(
    pool: &PgPool,
    post_id: i64,
    local_domain: &str,
    http_client: &reqwest::Client,
) -> Option<crate::seiran_post::SeiranPost> {
    let row = sqlx::query(
        "SELECT p.body, p.language, p.visibility::text AS visibility, p.content_warning,
                p.emoji_map, p.poll, p.ap_object_id, a.ap_uri
         FROM posts p
         JOIN actors a ON a.id = p.actor_id
         WHERE p.id = $1",
    )
    .bind(post_id)
    .fetch_optional(pool)
    .await
    .ok()??;

    let ap_uri: Option<String> = row.try_get("ap_uri").ok().flatten();
    let Some(counterpart_author_id) = ap_uri else {
        tracing::warn!(
            "[build_seiran_post_for_atp_commit] actorがap_uri未設定のためseiranPostを省略 post_id={}",
            post_id
        );
        return None;
    };

    let raw_body: String = row.try_get("body").unwrap_or_default();
    let (qualified_body, _mentions) =
        crate::mention::convert_mentions_for_ap(&raw_body, local_domain, pool, http_client).await;

    let (attachments, link_cards) =
        crate::seiran_post::fetch_attachments_and_link_cards(pool, post_id)
            .await
            .unwrap_or_default();

    Some(crate::seiran_post::SeiranPost {
        body: qualified_body,
        language: row.try_get("language").ok().flatten(),
        visibility: row
            .try_get("visibility")
            .unwrap_or_else(|_| "public".to_string()),
        content_warning: row.try_get("content_warning").ok().flatten(),
        emoji_map: row
            .try_get("emoji_map")
            .unwrap_or_else(|_| serde_json::json!({})),
        poll: row.try_get("poll").ok().flatten(),
        counterpart_post_id: row.try_get("ap_object_id").ok().flatten(),
        counterpart_author_id,
        attachments,
        link_cards,
    })
}

pub struct AtpCommitService {
    pool: PgPool,
    events: Arc<EventBus<AtpCommitEvent>>,
    http_client: Arc<reqwest::Client>,
    /// `seiranPost.body`（#237）のローカルメンション完全修飾（`build_seiran_post_for_atp_commit`
    /// 参照）に使う。AP側と表記を揃えるため`convert_mentions_for_bsky`ではなく
    /// `convert_mentions_for_ap`（Fediverse形式 `@user@domain`）を使う。
    local_domain: crate::LocalDomain,
}

/// `AtpCommitService::commit_like` でコミットする Like（`app.bsky.feed.like`）の内容。
#[derive(Clone, Copy)]
pub struct LikeCommit<'a> {
    /// 対象の`posts.id`。
    pub post_id: i64,
    pub target_at_uri: &'a str,
    pub target_at_cid: &'a str,
    /// 非標準の拡張フィールドとして載せる絵文字。
    pub emoji: Option<&'a str>,
    /// `reactions.id`（firehose で戻ってきた際の通知重複排除トークン）。
    pub reaction_id: i64,
}

/// `AtpCommitService::commit_profile` でコミットするプロフィール（`app.bsky.actor.profile`）。
/// 画像は（sha256_hex, mime_type, size）、ピン留め投稿は strongRef（uri, cid）。
pub struct ProfileCommit<'a> {
    pub display_name: &'a str,
    pub description: Option<&'a str>,
    pub avatar_media: Option<(String, String, i64)>,
    pub banner_media: Option<(String, String, i64)>,
    pub pinned_post: Option<(String, String)>,
}

impl<'a> ProfileCommit<'a> {
    /// 表示名のみのプロフィール（新規登録時）。
    pub fn new(display_name: &'a str) -> Self {
        Self {
            display_name,
            description: None,
            avatar_media: None,
            banner_media: None,
            pinned_post: None,
        }
    }
}

/// `AtpCommitService::commit_post` でコミットする投稿（`app.bsky.feed.post`）の内容。
pub struct PostCommit<'a> {
    /// 対応する`posts.id`（コミット後に`posts`の ATP 列を更新する）。
    pub post_id: i64,
    pub text: &'a str,
    pub facets: Vec<BskyFacet>,
    /// 画像・引用（`BskyEmbed::Record`でBskyネイティブ引用、`External`でURLカード）等。
    pub embed: Option<BskyEmbed>,
    /// リプライ投稿の`reply`フィールド。
    pub reply: Option<BskyPostReply>,
    /// 投稿の言語（`langs`フィールドに1件の配列として設定、`None`なら省略。Bskyのみの概念）。
    pub lang: Option<String>,
}

impl AtpCommitService {
    pub fn new(
        pool: PgPool,
        events: Arc<EventBus<AtpCommitEvent>>,
        http_client: Arc<reqwest::Client>,
        local_domain: crate::LocalDomain,
    ) -> Self {
        Self {
            pool,
            events,
            http_client,
            local_domain,
        }
    }

    pub fn events(&self) -> &Arc<EventBus<AtpCommitEvent>> {
        &self.events
    }

    fn publish_event(&self, event: AtpCommitEvent) {
        self.events.publish(event);
    }

    fn spawn_request_crawl(&self) {
        if let Ok(local_domain) = std::env::var("LOCAL_DOMAIN") {
            // ATP_RELAY_URL はカンマ区切りで複数指定でき、全てへ並行して requestCrawl する。
            // 未設定時は本番の Bsky 公式リレーのみ（従来通りの挙動）。ローカル調査用に自前の
            // relay 実装を並行稼働させる場合は
            // ATP_RELAY_URL=https://bsky.network,http://localhost:2470 のように追加する。
            let relay_base_raw = std::env::var("ATP_RELAY_URL")
                .unwrap_or_else(|_| "https://bsky.network".to_string());
            let relay_bases: Vec<String> = relay_base_raw
                .split(',')
                .map(|s| s.trim().trim_end_matches('/').to_string())
                .filter(|s| !s.is_empty())
                .collect();
            for relay_base in relay_bases {
                let http_client = Arc::clone(&self.http_client);
                let local_domain = local_domain.clone();
                tokio::spawn(async move {
                    let url = format!("{}/xrpc/com.atproto.sync.requestCrawl", relay_base);
                    match http_client
                        .post(&url)
                        .json(&serde_json::json!({"hostname": local_domain}))
                        .send()
                        .await
                    {
                        Ok(res) => tracing::info!("[atp] requestCrawl({}) → {}", url, res.status()),
                        Err(e) => tracing::error!("[atp] requestCrawl({}) 失敗: {}", url, e),
                    }
                });
            }
        }
    }

    /// 指定アクターの全 ATP レコードを MST 構築用エントリとしてロードする。
    async fn load_atp_entries(
        conn: &mut sqlx::PgConnection,
        actor_id: i64,
    ) -> Result<Vec<(String, Cid)>, AtpCommitError> {
        let post_rows = sqlx::query(
            "SELECT at_rkey, at_cid FROM posts
             WHERE actor_id = $1 AND at_rkey IS NOT NULL AND at_cid IS NOT NULL AND deleted_at IS NULL",
        )
        .bind(actor_id)
        .fetch_all(&mut *conn)
        .await?;

        let record_rows =
            sqlx::query("SELECT collection, rkey, cid FROM atp_records WHERE actor_id = $1")
                .bind(actor_id)
                .fetch_all(&mut *conn)
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

    /// レコードを追加・更新するコミット。`post_id` を指定すると、同じトランザクションで
    /// `posts.at_uri/at_cid/at_rkey` も更新する。
    async fn commit_record_inner(
        &self,
        actor_id: i64,
        record: CommitRecord,
        now: DateTime<Utc>,
        post_id: Option<i64>,
    ) -> Result<CommitResult, AtpCommitError> {
        self.apply_write(actor_id, &RepoWrite::Put(record), now, post_id)
            .await
    }

    /// 共通コミットパイプライン。リポジトリの先頭（`actors.at_repo_*`）とレコード一覧の
    /// 読み取りから書き込みまでを1つの SERIALIZABLE トランザクションで行い、直列化失敗なら
    /// 頭からやり直す。同じアクターへのコミットが並行すると（同一プロセス内でも、無停止更新で
    /// 新旧2プロセスが重なっている間でも）、両方が同じ prev から rev を作ってリポジトリが分岐する。
    async fn apply_write(
        &self,
        actor_id: i64,
        write: &RepoWrite,
        now: DateTime<Utc>,
        post_id: Option<i64>,
    ) -> Result<CommitResult, AtpCommitError> {
        let mut attempt = 1;
        loop {
            match self.try_apply_write(actor_id, write, now, post_id).await {
                Ok((result, frame)) => {
                    if let Some(frame) = frame {
                        self.publish_event(AtpCommitEvent {
                            frame_bytes: frame,
                            seq: result.seq,
                        });
                    }
                    return Ok(result);
                }
                Err(AtpCommitError::Db(e))
                    if crate::db::is_serialization_failure(&e)
                        && attempt < MAX_SERIALIZATION_RETRIES =>
                {
                    tracing::warn!(
                        "[atp] コミットが競合したためやり直します actor_id={} attempt={}/{}: {}",
                        actor_id,
                        attempt,
                        MAX_SERIALIZATION_RETRIES,
                        e
                    );
                    tokio::time::sleep(Duration::from_millis(20 * u64::from(attempt))).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// MST 構築 → commit 署名 → CAR 生成 → atp_blocks 保存 → actors 更新
    /// → atp_records 反映 → atp_repo_events 記録までを1トランザクションで行い、
    /// コミット結果と subscribeRepos へ流すフレームを返す。
    async fn try_apply_write(
        &self,
        actor_id: i64,
        write: &RepoWrite,
        now: DateTime<Utc>,
        post_id: Option<i64>,
    ) -> Result<(CommitResult, Option<Vec<u8>>), AtpCommitError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
            .execute(&mut *tx)
            .await?;

        // ① アクター情報取得
        let actor_row = sqlx::query(
            "SELECT at_did, at_signing_key_pem, at_repo_cid, at_repo_rev, at_repo_data_cid
             FROM actors WHERE id = $1",
        )
        .bind(actor_id)
        .fetch_one(&mut *tx)
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
        let prev_data_cid_str: Option<String> =
            actor_row.try_get::<Option<String>, _>("at_repo_data_cid")?;

        // ② 署名鍵をロード
        let signing_key = signing_key_from_pem(&signing_key_pem)?;

        // ③ 既存エントリをロードして対象キーを差し替える
        // 同一キー（例: app.bsky.actor.profile/self の再コミット）が既に存在する場合は
        // 古いエントリを取り除いてから積む。取り除かずに push すると MST に同一キーが
        // 2つ入ってしまい、AppView がリジェクトする不正な木になる。
        let mut entries = Self::load_atp_entries(&mut tx, actor_id).await?;
        let entry_key = format!("{}/{}", write.collection(), write.rkey());
        entries.retain(|(k, _)| k != &entry_key);
        if let RepoWrite::Put(record) = write {
            entries.push((entry_key.clone(), record.cid));
        }
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));

        // ④ MST 構築
        let (mst_root, mst_blocks) = build_mst(&entries)?;

        // ⑤ commit 生成・P-256 署名
        let new_rev = generate_tid();
        let prev_cid_parsed = prev_commit_cid_str
            .as_deref()
            .and_then(|s| cid_from_str(s).ok());
        let prev_data_cid_parsed = prev_data_cid_str
            .as_deref()
            .and_then(|s| cid_from_str(s).ok());
        let (commit_cid, commit_cbor) =
            create_commit(&at_did, &new_rev, mst_root, prev_cid_parsed, &signing_key)?;

        // ⑥ CAR エンコード（削除ではレコードのブロックを含まない）
        let mut new_blocks = mst_blocks;
        if let RepoWrite::Put(record) = write {
            new_blocks.push((record.cid, record.cbor.clone()));
        }
        new_blocks.push((commit_cid, commit_cbor));
        let diff_car = encode_car(&commit_cid, &new_blocks)?;

        let commit_cid_str = cid_to_string(&commit_cid);
        let mst_root_cid_str = cid_to_string(&mst_root);

        // ⑦ atp_blocks INSERT
        insert_blocks(&mut tx, actor_id, &new_blocks).await?;

        // ⑧ actors UPDATE
        sqlx::query("UPDATE actors SET at_repo_cid = $1, at_repo_rev = $2, at_repo_data_cid = $3 WHERE id = $4")
            .bind(&commit_cid_str)
            .bind(&new_rev)
            .bind(&mst_root_cid_str)
            .bind(actor_id)
            .execute(&mut *tx)
            .await?;

        // ⑨ atp_records 反映
        let (ops_json, ws_ops, blob_cids) = match write {
            RepoWrite::Put(record) => {
                let record_cid_str = cid_to_string(&record.cid);
                // 投稿は posts テーブルで管理するため atp_records に入れない。入れると
                // load_atp_entries で posts テーブルとの二重取得になり MST に重複キーが生じて
                // AppView に拒否される。
                if record.collection != "app.bsky.feed.post" {
                    sqlx::query(
                        "INSERT INTO atp_records (actor_id, collection, rkey, cid) VALUES ($1, $2, $3, $4)
                         ON CONFLICT (actor_id, collection, rkey) DO UPDATE SET cid = EXCLUDED.cid",
                    )
                    .bind(actor_id)
                    .bind(&record.collection)
                    .bind(&record.rkey)
                    .bind(&record_cid_str)
                    .execute(&mut *tx)
                    .await?;
                }
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
                (
                    serde_json::json!([{
                        "action": record.action,
                        "path": entry_key,
                        "cid": record_cid_str,
                    }]),
                    vec![CommitEvtOp {
                        action: record.action.to_string(),
                        path: entry_key,
                        cid: Some(record.cid),
                    }],
                    record.blob_cids.as_slice(),
                )
            }
            RepoWrite::Delete { collection, rkey } => {
                sqlx::query(
                    "DELETE FROM atp_records WHERE actor_id = $1 AND collection = $2 AND rkey = $3",
                )
                .bind(actor_id)
                .bind(collection)
                .bind(rkey)
                .execute(&mut *tx)
                .await?;
                (
                    serde_json::json!([{"action": "delete", "path": entry_key}]),
                    vec![CommitEvtOp {
                        action: "delete".to_string(),
                        path: entry_key,
                        cid: None,
                    }],
                    &[][..],
                )
            }
        };

        // ⑩ atp_repo_events INSERT → seq 取得
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
        let frame_opt = build_commit_frame(&crate::atp::repo::CommitFrame {
            seq,
            did: &at_did,
            commit_cid: &commit_cid,
            prev_cid: prev_cid_parsed.as_ref(),
            rev: &new_rev,
            since: prev_rev.as_deref(),
            car_bytes: &diff_car,
            ops: &ws_ops,
            blob_cids,
            time: &time_str,
            prev_data: prev_data_cid_parsed.as_ref(),
        })
        .ok();
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

        Ok((
            CommitResult {
                commit_cid,
                rev: new_rev,
                seq,
                at_did,
            },
            frame_opt,
        ))
    }

    /// 外部ATクライアントの `com.atproto.repo.createRecord`/`putRecord` 用。
    /// 任意コレクションのレコードJSONをそのままコミットする。`app.bsky.feed.post` は
    /// 画像/動画embed組み立て等の投稿専用ロジックを経由する `commit_post` を使うこと
    /// （このメソッドはレコード値をそのままDAG-CBORエンコードするだけで、`posts` テーブルの
    /// 更新も行わない）。
    /// 戻り値は `(コミット結果, 作成したレコード自体のCID)`。`CommitResult::commit_cid` は
    /// リポジトリ全体のcommit CIDであり、`createRecord`/`putRecord` のレスポンスが返すべき
    /// 「レコードのCID」とは別物のため、レコードCIDを別途返す。
    pub async fn commit_generic_record(
        &self,
        actor_id: i64,
        collection: String,
        rkey: String,
        value: &serde_json::Value,
        action: &'static str,
        now: DateTime<Utc>,
    ) -> Result<(CommitResult, Cid), AtpCommitError> {
        let (cbor, cid) = encode_generic_record(value)?;
        let blob_cids = collect_blob_cids(value);
        let record = CommitRecord {
            collection,
            rkey,
            cbor,
            cid,
            action,
            blob_cids,
        };
        let result = self
            .commit_record_inner(actor_id, record, now, None)
            .await?;
        self.spawn_request_crawl();
        Ok((result, cid))
    }

    /// `com.atproto.repo.createRecord`/`putRecord` で `app.bsky.feed.post` を受けた場合の
    /// 専用コミット。クライアントが既に完成させた `record` JSON（text/facets/embed/reply等）を
    /// そのままDAG-CBORエンコードしてコミットする点が `commit_post` と異なる（`commit_post` は
    /// seiranネイティブ投稿API向けに、DB上の添付情報から embed を再構築する）。`posts` テーブルの
    /// `at_uri`/`at_cid`/`at_rkey` 更新は `commit_record_inner` に `post_id` を渡すことで行う。
    pub async fn commit_post_record(
        &self,
        actor_id: i64,
        post_id: i64,
        rkey: String,
        value: &serde_json::Value,
        action: &'static str,
        now: DateTime<Utc>,
    ) -> Result<(CommitResult, Cid), AtpCommitError> {
        let (cbor, cid) = encode_generic_record(value)?;
        let blob_cids = collect_blob_cids(value);
        let record = CommitRecord {
            collection: "app.bsky.feed.post".to_string(),
            rkey,
            cbor,
            cid,
            action,
            blob_cids,
        };
        let result = self
            .commit_record_inner(actor_id, record, now, Some(post_id))
            .await?;
        self.spawn_request_crawl();
        Ok((result, cid))
    }

    /// ポスト作成コミット（posts テーブル更新を追加）。通常投稿・引用投稿（`embed` に
    /// `BskyEmbed::Record`/`External`）・CW 投稿など、`app.bsky.feed.post` を作る全経路で使う
    pub async fn commit_post(
        &self,
        actor_id: i64,
        post: PostCommit<'_>,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        let PostCommit {
            post_id,
            text,
            facets,
            embed,
            reply,
            lang,
        } = post;
        let rkey = generate_tid();
        let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let seiran_post = build_seiran_post_for_atp_commit(
            &self.pool,
            post_id,
            self.local_domain.as_str(),
            &self.http_client,
        )
        .await;
        let blob_cids = blob_cids_for_embed(&embed);
        let (record_cbor, record_cid) = encode_bsky_feed_post(
            text,
            &created_at_str,
            facets,
            embed,
            reply,
            lang,
            seiran_post,
        )?;
        let record_cid_str = cid_to_string(&record_cid);

        let record = CommitRecord {
            collection: "app.bsky.feed.post".to_string(),
            rkey: rkey.clone(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
            blob_cids,
        };

        let result = self
            .commit_record_inner(actor_id, record, now, Some(post_id))
            .await?;

        let at_uri = format!("at://{}/app.bsky.feed.post/{}", result.at_did, rkey);
        tracing::info!(
            "[atp] commit 完了: at_uri={}, cid={}",
            at_uri,
            record_cid_str
        );
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
            collection: "app.bsky.feed.repost".to_string(),
            rkey: rkey.clone(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
            blob_cids: vec![],
        };

        self.commit_record_inner(actor_id, record, now, None)
            .await?;

        if let Some(pid) = post_id {
            sqlx::query("UPDATE posts SET atp_repost_rkey = $1 WHERE id = $2")
                .bind(&rkey)
                .bind(pid)
                .execute(&self.pool)
                .await?;
        }

        tracing::info!(
            "[atp] repost commit 完了: actor_id={}, rkey={}",
            actor_id,
            rkey
        );
        self.spawn_request_crawl();
        Ok(())
    }

    /// `app.bsky.feed.like` レコードをコミットする（リアクション連携）。
    /// ATP には絵文字リアクションの概念が無いため、どの絵文字でも Like として送る。
    /// `emoji` は非標準の拡張フィールドとしてベストエフォートで載せる。
    /// `reaction_id`（`reactions.id`）も非標準拡張フィールドとして載せ、このLikeが自分自身の
    /// firehose経由で戻ってきた際に通知の重複排除トークンとして使う（`docs/protocols.md` 8節）。
    /// 成功したら生成した at_uri（`at://did/app.bsky.feed.like/rkey`）を
    /// `reactions.at_uri` に自己保存する（`commit_repost` が `posts.atp_repost_rkey` を
    /// 自己保存するのと同じ流儀）。切替（別の絵文字への変更）の場合、旧 Like の削除は
    /// 呼び出し側が事前に `delete_atp_like` で行う（このメソッドは新規作成のみを担う）。
    pub async fn commit_like(
        &self,
        actor_id: i64,
        like: &LikeCommit<'_>,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        let LikeCommit {
            post_id,
            target_at_uri,
            target_at_cid,
            emoji,
            reaction_id,
        } = *like;
        let rkey = generate_tid();
        let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let (record_cbor, record_cid) = encode_bsky_feed_like(
            target_at_uri,
            target_at_cid,
            &created_at_str,
            emoji,
            reaction_id,
        )?;

        let record = CommitRecord {
            collection: "app.bsky.feed.like".to_string(),
            rkey: rkey.clone(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
            blob_cids: vec![],
        };

        let result = self
            .commit_record_inner(actor_id, record, now, None)
            .await?;

        let at_uri_self = format!("at://{}/app.bsky.feed.like/{}", result.at_did, rkey);
        sqlx::query("UPDATE reactions SET at_uri = $1 WHERE post_id = $2 AND actor_id = $3")
            .bind(&at_uri_self)
            .bind(post_id)
            .bind(actor_id)
            .execute(&self.pool)
            .await?;

        tracing::info!(
            "[atp] like commit 完了: actor_id={}, rkey={}",
            actor_id,
            rkey
        );
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
            collection: "app.bsky.graph.follow".to_string(),
            rkey: rkey.clone(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
            blob_cids: vec![],
        };

        self.commit_record_inner(actor_id, record, now, None)
            .await?;

        tracing::info!(
            "[atp] follow commit 完了: actor_id={}, subject={}, rkey={}",
            actor_id,
            subject_did,
            rkey
        );
        self.spawn_request_crawl();
        Ok(rkey)
    }

    /// `app.bsky.graph.block` レコードをコミットする（Bsky準拠ブロック機能）。
    /// 成功時は生成した rkey を返す（アンブロック時のレコード削除に必要）。
    pub async fn commit_block(
        &self,
        actor_id: i64,
        subject_did: &str,
        now: DateTime<Utc>,
    ) -> Result<String, AtpCommitError> {
        let rkey = generate_tid();
        let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let (record_cbor, record_cid) = encode_bsky_graph_block(subject_did, &created_at_str)?;

        let record = CommitRecord {
            collection: "app.bsky.graph.block".to_string(),
            rkey: rkey.clone(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
            blob_cids: vec![],
        };

        self.commit_record_inner(actor_id, record, now, None)
            .await?;

        tracing::info!(
            "[atp] block commit 完了: actor_id={}, subject={}, rkey={}",
            actor_id,
            subject_did,
            rkey
        );
        self.spawn_request_crawl();
        Ok(rkey)
    }

    /// `app.bsky.graph.block` レコードを MST から削除する（アンブロック時）。
    pub async fn commit_delete_block(
        &self,
        actor_id: i64,
        rkey: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        self.delete_atp_record_generic(actor_id, "app.bsky.graph.block", rkey, now)
            .await
    }

    /// `app.bsky.graph.list` レコードをコミットする（リスト機能 #63、公開リストのみ呼ぶ）。
    /// 成功時は `(rkey, at_uri, cid)` を返す。呼び出し元が `ListRepository::set_atp_list_record`
    /// で `lists` テーブルに保存する。
    pub async fn commit_graph_list(
        &self,
        actor_id: i64,
        name: &str,
        now: DateTime<Utc>,
    ) -> Result<(String, String, String), AtpCommitError> {
        let rkey = generate_tid();
        let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let (record_cbor, record_cid) = encode_bsky_graph_list(name, &created_at_str)?;
        let cid_str = cid_to_string(&record_cid);

        let record = CommitRecord {
            collection: "app.bsky.graph.list".to_string(),
            rkey: rkey.clone(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
            blob_cids: vec![],
        };

        let result = self
            .commit_record_inner(actor_id, record, now, None)
            .await?;
        let at_uri = format!("at://{}/app.bsky.graph.list/{}", result.at_did, rkey);

        tracing::info!(
            "[atp] list commit 完了: actor_id={}, rkey={}",
            actor_id,
            rkey
        );
        self.spawn_request_crawl();
        Ok((rkey, at_uri, cid_str))
    }

    /// `app.bsky.graph.listitem` レコードをコミットする（公開リストの、かつ Bsky 可視の
    /// メンバー追加時のみ呼ぶ）。成功時は `(rkey, at_uri)` を返す。
    pub async fn commit_graph_listitem(
        &self,
        actor_id: i64,
        list_uri: &str,
        subject_did: &str,
        now: DateTime<Utc>,
    ) -> Result<(String, String), AtpCommitError> {
        let rkey = generate_tid();
        let created_at_str = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

        let (record_cbor, record_cid) =
            encode_bsky_graph_listitem(list_uri, subject_did, &created_at_str)?;

        let record = CommitRecord {
            collection: "app.bsky.graph.listitem".to_string(),
            rkey: rkey.clone(),
            cbor: record_cbor,
            cid: record_cid,
            action: "create",
            blob_cids: vec![],
        };

        let result = self
            .commit_record_inner(actor_id, record, now, None)
            .await?;
        let at_uri = format!("at://{}/app.bsky.graph.listitem/{}", result.at_did, rkey);

        tracing::info!(
            "[atp] listitem commit 完了: actor_id={}, list={}, subject={}, rkey={}",
            actor_id,
            list_uri,
            subject_did,
            rkey
        );
        self.spawn_request_crawl();
        Ok((rkey, at_uri))
    }

    /// リポスト解除コミット（app.bsky.feed.repost レコードを MST から削除する）。
    pub async fn delete_atp_repost(
        &self,
        actor_id: i64,
        rkey: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        self.delete_atp_record_generic(actor_id, "app.bsky.feed.repost", rkey, now)
            .await
    }

    /// `app.bsky.feed.post` レコードを削除する。
    /// Fedi リモートポストのリポスト時に作るフォールバック投稿（`commit_post` が
    /// 本文「🔁」と元ポストURLのexternal embedをコミットし、
    /// `posts.at_rkey` に自己保存したもの）を、リポスト取り消し時に retract するために使う。
    pub async fn delete_atp_post(
        &self,
        actor_id: i64,
        rkey: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        self.delete_atp_record_generic(actor_id, "app.bsky.feed.post", rkey, now)
            .await
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
        self.delete_atp_record_generic(actor_id, "app.bsky.feed.like", rkey, now)
            .await
    }

    /// `app.bsky.graph.follow` レコードを削除コミットする（アンフォロー）。
    pub async fn commit_delete_follow(
        &self,
        actor_id: i64,
        rkey: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        self.delete_atp_record_generic(actor_id, "app.bsky.graph.follow", rkey, now)
            .await
    }

    /// 指定コレクションのレコードを MST から削除する。
    pub async fn delete_atp_record_generic(
        &self,
        actor_id: i64,
        collection: &str,
        rkey: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        let write = RepoWrite::Delete {
            collection: collection.to_string(),
            rkey: rkey.to_string(),
        };
        self.apply_write(actor_id, &write, now, None).await?;

        tracing::info!(
            "[atp] {} delete commit 完了: actor_id={}, rkey={}",
            collection,
            actor_id,
            rkey
        );
        self.spawn_request_crawl();
        Ok(())
    }

    /// `app.bsky.graph.list` レコードを削除する（リスト非公開化・削除時）。
    pub async fn delete_atp_graph_list(
        &self,
        actor_id: i64,
        rkey: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        self.delete_atp_record_generic(actor_id, "app.bsky.graph.list", rkey, now)
            .await
    }

    /// `app.bsky.graph.listitem` レコードを削除する（メンバー削除・リスト削除時）。
    pub async fn delete_atp_graph_listitem(
        &self,
        actor_id: i64,
        rkey: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        self.delete_atp_record_generic(actor_id, "app.bsky.graph.listitem", rkey, now)
            .await
    }

    /// プロフィール（再）コミット。新規登録時は avatar/description/pinned_post なしで呼ばれ、
    /// プロフィール編集時は bio・アバター blob 情報を渡して再コミットする。
    /// `pinned_post` はピン留め投稿への strongRef（uri, cid）。ピン留めが無い/ピン留め投稿が
    /// Bsky 側に存在しない場合は `None` を渡す（#61）。
    /// 既に `app.bsky.actor.profile/self` が存在するかで action(create/update)を自動判定する。
    pub async fn commit_profile(
        &self,
        actor_id: i64,
        profile: &ProfileCommit<'_>,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        let ProfileCommit {
            display_name,
            description,
            avatar_media,
            banner_media,
            pinned_post,
        } = profile;
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
        let banner_ref = banner_media
            .as_ref()
            .map(|(sha256, mime, size)| (sha256.as_str(), mime.as_str(), *size));
        let pinned_post_ref = pinned_post
            .as_ref()
            .map(|(uri, cid)| (uri.as_str(), cid.as_str()));
        let (record_cbor, record_cid) = encode_bsky_actor_profile(
            display_name,
            *description,
            avatar_ref,
            banner_ref,
            pinned_post_ref,
            &created_at_str,
        )?;

        let record = CommitRecord {
            collection: "app.bsky.actor.profile".to_string(),
            rkey: "self".to_string(),
            cbor: record_cbor,
            cid: record_cid,
            action,
            blob_cids: vec![],
        };

        let result = self
            .commit_record_inner(actor_id, record, now, None)
            .await?;

        tracing::info!(
            "[atp] profile commit 完了（{}）: did={}",
            action,
            result.at_did
        );
        self.spawn_request_crawl();
        Ok(())
    }

    /// `chat.bsky.actor.declaration`（Bsky DM受信可否設定、rkey固定`self`）をコミットする。
    /// このレコードが無いと、Bluesky公式クライアントは相手（seiranユーザー）へのDM送信を
    /// 保守的にブロックする（`docs/protocols.md` 9節）。新規登録時・既存ユーザーへの
    /// バックフィルの両方から呼ばれる。既に同じ値で存在する場合も冪等に再コミットしてよい
    /// （呼び出し頻度は低いためコストは無視できる）。
    pub async fn commit_chat_declaration(
        &self,
        actor_id: i64,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        let existing: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM atp_records \
             WHERE actor_id = $1 AND collection = 'chat.bsky.actor.declaration' AND rkey = 'self')",
        )
        .bind(actor_id)
        .fetch_one(&self.pool)
        .await?;
        let action: &'static str = if existing { "update" } else { "create" };

        let (record_cbor, record_cid) = encode_chat_actor_declaration("all")?;

        let record = CommitRecord {
            collection: "chat.bsky.actor.declaration".to_string(),
            rkey: "self".to_string(),
            cbor: record_cbor,
            cid: record_cid,
            action,
            blob_cids: vec![],
        };

        let result = self
            .commit_record_inner(actor_id, record, now, None)
            .await?;

        tracing::info!(
            "[atp] chat declaration commit 完了（{}）: did={}",
            action,
            result.at_did
        );
        self.spawn_request_crawl();
        Ok(())
    }

    /// `org.seiran.actor.declaration`（リモートseiranアクターの相互申告マージ用、
    /// rkey固定`self`、#236）をコミットする。ローカルユーザー登録時（DID発行と同時）、
    /// および起動時のバックフィル（`commit_chat_declaration`と同様のパターン）から呼ぶ。
    pub async fn commit_seiran_actor_declaration(
        &self,
        actor_id: i64,
        ap_actor_uri: &str,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        let existing: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM atp_records \
             WHERE actor_id = $1 AND collection = 'org.seiran.actor.declaration' AND rkey = 'self')",
        )
        .bind(actor_id)
        .fetch_one(&self.pool)
        .await?;
        let action: &'static str = if existing { "update" } else { "create" };

        let (record_cbor, record_cid) = encode_seiran_actor_declaration(ap_actor_uri)?;

        let record = CommitRecord {
            collection: "org.seiran.actor.declaration".to_string(),
            rkey: "self".to_string(),
            cbor: record_cbor,
            cid: record_cid,
            action,
            blob_cids: vec![],
        };

        let result = self
            .commit_record_inner(actor_id, record, now, None)
            .await?;

        tracing::info!(
            "[atp] seiran actor declaration commit 完了（{}）: did={}",
            action,
            result.at_did
        );
        self.spawn_request_crawl();
        Ok(())
    }

    /// `app.bsky.actor.contentVisibilityDeclaration`（Discoverフィード等のアルゴリズム
    /// レコメンドからの除外要求、rkey固定`self`）をコミットする。設定画面のプライバシー
    /// セクションからトグルするたびに呼ばれる（毎回 create/update を判定して冪等にコミット）。
    pub async fn commit_content_visibility(
        &self,
        actor_id: i64,
        hide_from_algorithmic_recommendations: bool,
        now: DateTime<Utc>,
    ) -> Result<(), AtpCommitError> {
        let existing: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM atp_records \
             WHERE actor_id = $1 AND collection = 'app.bsky.actor.contentVisibilityDeclaration' AND rkey = 'self')",
        )
        .bind(actor_id)
        .fetch_one(&self.pool)
        .await?;
        let action: &'static str = if existing { "update" } else { "create" };

        let (record_cbor, record_cid) =
            encode_content_visibility_declaration(hide_from_algorithmic_recommendations)?;

        let record = CommitRecord {
            collection: "app.bsky.actor.contentVisibilityDeclaration".to_string(),
            rkey: "self".to_string(),
            cbor: record_cbor,
            cid: record_cid,
            action,
            blob_cids: vec![],
        };

        let result = self
            .commit_record_inner(actor_id, record, now, None)
            .await?;

        tracing::info!(
            "[atp] content visibility declaration commit 完了（{}）: did={}, hide={}",
            action,
            result.at_did,
            hide_from_algorithmic_recommendations
        );
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
        let compressed =
            zstd::encode_all(&frame[..], 3).map_err(|e| RepoError::Cbor(e.to_string()))?;
        sqlx::query("UPDATE atp_repo_events SET frame_bytes = $1 WHERE id = $2")
            .bind(&compressed)
            .bind(seq)
            .execute(&self.pool)
            .await?;

        self.publish_event(AtpCommitEvent {
            frame_bytes: frame,
            seq,
        });

        tracing::info!(
            "[atp] identity broadcast: did={}, handle={}, seq={}",
            did,
            handle,
            seq
        );
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
        let compressed =
            zstd::encode_all(&frame[..], 3).map_err(|e| RepoError::Cbor(e.to_string()))?;
        sqlx::query("UPDATE atp_repo_events SET frame_bytes = $1 WHERE id = $2")
            .bind(&compressed)
            .bind(seq)
            .execute(&self.pool)
            .await?;

        self.publish_event(AtpCommitEvent {
            frame_bytes: frame,
            seq,
        });

        tracing::info!(
            "[atp] account broadcast: did={}, active={}, seq={}",
            did,
            active,
            seq
        );
        Ok(())
    }
}

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgPool;

/// 新規ローカルアクター（`ActorRepository::insert_local`・`create_local_account`の入力）。
/// `at_did`/`at_signing_key_pem`/`at_rotation_key_pem`は、自ホストドメインが未確定
/// （シングルホストモード）でPLC genesisを行っていない場合は`None`になる。
#[derive(Debug, Clone, Copy)]
pub struct NewLocalActor<'a> {
    pub id: i64,
    pub username: &'a str,
    pub domain: &'a str,
    pub at_did: Option<&'a str>,
    pub at_signing_key_pem: Option<&'a str>,
    pub at_rotation_key_pem: Option<&'a str>,
    pub birth_date: Option<NaiveDate>,
}

/// ローカルアクター行を挿入する（`insert_local`・`create_local_account`共通のSQL）。
pub(crate) async fn insert_local_actor_row(
    conn: &mut sqlx::PgConnection,
    user_id: i64,
    a: &NewLocalActor<'_>,
) -> Result<(), sqlx::Error> {
    // ap_uri を格納しておくことで、万一リモートActor解決処理が自ドメインURIを
    // 誤って渡してきても find_by_ap_uri / upsert_remote_fedi の ON CONFLICT (ap_uri)
    // による自然な重複排除が効く（#110 の防御的二重チェック）。
    let ap_uri = format!("https://{}/users/{}", a.domain, a.username);
    sqlx::query(
        "INSERT INTO actors (id, user_id, actor_type, username, domain, ap_uri, at_did, at_signing_key_pem, at_rotation_key_pem, birth_date, created_at, updated_at)
         VALUES ($1, $2, 'local', $3, $4, $5, $6, $7, $8, $9, NOW(), NOW())",
    )
    .bind(a.id)
    .bind(user_id)
    .bind(a.username)
    .bind(a.domain)
    .bind(&ap_uri)
    .bind(a.at_did)
    .bind(a.at_signing_key_pem)
    .bind(a.at_rotation_key_pem)
    .bind(a.birth_date)
    .execute(conn)
    .await
    .map(|_| ())
}

/// リモートFediアクターのプロフィール（AP Actor 文書から
/// `FediActorProfile::from_ap_actor`（`crate::ap::client`）で組み立てる）。
/// `upsert_remote_fedi`・`seiran_actor_merge::discover_fedi_actor`の入力。
#[derive(Debug, Clone)]
pub struct FediActorProfile {
    pub ap_uri: String,
    pub ap_inbox_url: String,
    pub username: String,
    pub domain: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub banner_url: Option<String>,
    /// 自己紹介文（AP Person の summary を投稿本文と同じallowlistでサニタイズしたHTML）。
    /// `None` の場合は既存値を保持する。
    pub bio: Option<String>,
    /// 表示名中のカスタム絵文字（`:shortcode:`）→画像URLのマップ（AP Person の tag 配列由来）。
    pub emoji_map: serde_json::Value,
    /// プロフィールのキーバリュー項目（#62、AP Actor の `attachment` `PropertyValue` 由来）。
    pub profile_fields: serde_json::Value,
    /// AP拡張フィールド`seiranAtDid`での ATP 側の相手の自己申告（未確認、#236）。
    pub claimed_at_did: Option<String>,
}

/// リモートBskyアクターのプロフィール（AppView `getProfile` 等の取得結果）。
/// `upsert_remote_bsky`の入力。
#[derive(Debug, Clone, Copy)]
pub struct BskyActorProfile<'a> {
    pub at_did: &'a str,
    pub handle: &'a str,
    pub display_name: Option<&'a str>,
    pub avatar_url: Option<&'a str>,
    pub banner_url: Option<&'a str>,
}

/// ローカルアクターのプロフィール更新内容（`ActorRepository::update_profile`の入力、全項目を上書きする）。
#[derive(Debug, Clone, Copy)]
pub struct LocalProfileUpdate<'a> {
    pub display_name: Option<&'a str>,
    pub bio: Option<&'a str>,
    pub avatar_media_id: Option<i64>,
    pub banner_media_id: Option<i64>,
    pub profile_fields: &'a serde_json::Value,
    /// `display_name`から解決済みのショートコード→URLマップ（#186）。
    pub emoji_map: &'a serde_json::Value,
    pub birth_date: Option<NaiveDate>,
    pub birth_date_public: bool,
}

/// `actors` テーブルの 1 行（アプリで使用するカラムのみ）。
///
/// PostgreSQL の `actor_type_enum` は SELECT 時に `::text` キャストして `String` に
/// デコードする（`ACTOR_COLS` 参照）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Actor {
    pub id: i64,
    pub user_id: Option<i64>,
    pub actor_type: String,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub ap_uri: Option<String>,
    pub ap_inbox_url: Option<String>,
    pub at_did: Option<String>,
    /// AT Protocolハンドル（`user.pds-domain`形式）。`bsky`型は常に最新値、`remote_seiran`型は
    /// マージ後も（`username`と異なり）Bsky側発見のたびに更新され続ける（#236拡張）。
    pub at_handle: Option<String>,
    pub at_repo_cid: Option<String>,
    pub at_repo_rev: Option<String>,
    pub at_signing_key_pem: Option<String>,
    pub bio: Option<String>,
    /// リモート seiran の対の行（魂の結合済み判定に使用）。
    pub seiran_pair_actor_id: Option<i64>,
    /// ブリッジユーザーの実ユーザーの行 ID。
    pub bridge_real_actor_id: Option<i64>,
    /// 表示名中のカスタム絵文字（`:shortcode:`）→画像URLマップ（Fedi受信、AP `tag` 配列由来）。
    pub emoji_map: Option<serde_json::Value>,
    /// プロフィールのキーバリュー項目（#62）。`[{"name": ..., "value": ...}, ...]`（最大
    /// `MAX_PROFILE_FIELDS` 件）。ローカルユーザーが編集した値、またはリモート Fedi アクター
    /// の AP Actor `attachment`（`type: "PropertyValue"`）から取り込んだ値。
    pub profile_fields: Option<serde_json::Value>,
    /// 生年月日（Misskey互換の`birthday`プロフィール項目）。
    pub birth_date: Option<NaiveDate>,
    /// `true`ならFediverseへ`vcard:bday`として公開する（デフォルト`false`、Misskey本家には
    /// この可視性切り替え自体が無くseiran独自の拡張）。
    pub birth_date_public: bool,
    /// フォロー承認制（鍵アカウント）。`true`なら新規フォローリクエストは`follows.status`が
    /// `pending`のまま留まり、本人の承認/拒否を待つ（ローカルユーザーのみ意味を持つ）。
    pub is_locked: bool,
    /// リモートseiranアクターの相互申告マージ用（#236）。`bsky`型の行が自己申告する
    /// 「自分のAP Actor URIはこれだ」という未確認の値。相互一致が確認できたら`ap_uri`
    /// 本体に確定させ、こちらはNULLに戻す。
    pub claimed_ap_uri: Option<String>,
    /// 同上。`fedi`型の行が自己申告する「自分のAT DIDはこれだ」という未確認の値。
    pub claimed_at_did: Option<String>,
    /// 退会済み日時（#242）。`Some` なら以降このアクターはユーザー向けの表示・検索・
    /// フォロー一覧等から除外すべき（内部処理・連合への削除通知はこの値を前提に動くため、
    /// この関数自体ではフィルタしない。呼び出し元で判定すること）。
    pub withdrawn_at: Option<DateTime<Utc>>,
    /// 凍結日時（管理者・モデレーターによるユーザー凍結）。`Some` なら以降このアクターの
    /// 新規アクティビティ（AP/ATP双方）は拒絶・非保存となり、`actor_is_hidden_for_viewer`
    /// 経由でタイムライン・通知・検索等の表示からも除外される。ローカル・リモート共通。
    pub suspended_at: Option<DateTime<Utc>>,
    /// アカウント単位のPLCローテーションキー（転出元API対応の前提、Phase A）。`None`は
    /// 未バックフィル（ジェネシス作成の旧アカウント）または既存DID転入済みアカウント
    /// （転入元PDSの鍵をそのまま維持していた旧仕様の名残り）を意味する。
    pub at_rotation_key_pem: Option<String>,
    /// DID転出済み日時。`is_suspended`（凍結）や転入フローの`migration_status`ゲートとも
    /// 異なる第三の状態で、以降はタイムライン等の読み取りのみ可能、書き込み系操作は
    /// 全て不可（`submitPlcOperation`成功時、または`deactivateAccount`呼び出し時に設定）。
    pub did_moved_out_at: Option<DateTime<Utc>>,
}

/// `Actor` の全フィールドに対応する SELECT カラム列。`actor_type` は enum のため text にキャストする。
const ACTOR_COLS: &str = "id, user_id, actor_type::text AS actor_type, username, domain, \
    display_name, ap_uri, ap_inbox_url, at_did, at_handle, at_repo_cid, at_repo_rev, at_signing_key_pem, \
    bio, seiran_pair_actor_id, bridge_real_actor_id, emoji_map, profile_fields, \
    birth_date, birth_date_public, is_locked, claimed_ap_uri, claimed_at_did, withdrawn_at, suspended_at, \
    at_rotation_key_pem, did_moved_out_at";

/// プロフィール編集画面（`PATCH /api/users/me/profile`）が読み書きする行の部分集合。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ActorProfileRow {
    pub id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_media_id: Option<i64>,
    pub banner_media_id: Option<i64>,
    pub profile_fields: serde_json::Value,
    /// 表示名中のカスタム絵文字（`:shortcode:`）→画像URLマップ（#186、ローカルアクターは
    /// `display_name` 変更のたびに `update_profile` が再計算・保存する）。
    pub emoji_map: Option<serde_json::Value>,
    /// 生年月日（Misskey互換の`birthday`プロフィール項目）。
    pub birth_date: Option<NaiveDate>,
    /// `true`ならFediverseへ`vcard:bday`として公開する（デフォルト`false`、Misskey本家には
    /// この可視性切り替え自体が無くseiran独自の拡張）。
    pub birth_date_public: bool,
}

#[async_trait]
pub trait ActorRepository: Send + Sync {
    /// ローカルユーザー（`actor_type = 'local'`）のアクターを user_id で取得する。
    async fn find_local_by_user_id(&self, user_id: i64) -> Result<Option<Actor>, sqlx::Error>;

    /// ユーザー名 + ドメインでアクターを取得する（退会済み〔`withdrawn_at`設定済み〕アクターは
    /// 除外する。ユーザー向けの表示・検索・新規フォロー解決等はこちらを使うこと。内部処理
    /// （AP受信ジョブ・連合への削除通知等、退会済みアクターも扱う必要がある処理）は
    /// `find_including_withdrawn_by_username_domain` を使うこと、#242）。
    async fn find_by_username_domain(
        &self,
        username: &str,
        domain: &str,
    ) -> Result<Option<Actor>, sqlx::Error>;

    /// `find_by_username_domain` の退会済みアクターを除外しない版。関数名をあえて不自然に
    /// することで、呼び出し側に「本当にこれでよいか」を意識させる（#242）。
    async fn find_including_withdrawn_by_username_domain(
        &self,
        username: &str,
        domain: &str,
    ) -> Result<Option<Actor>, sqlx::Error>;

    /// ActivityPub Actor URI でアクターを取得する。
    async fn find_by_ap_uri(&self, ap_uri: &str) -> Result<Option<Actor>, sqlx::Error>;

    /// AT Protocol DID でアクターを取得する。
    async fn find_by_did(&self, did: &str) -> Result<Option<Actor>, sqlx::Error>;

    /// `at_did IS NOT NULL` なアクター（＝ATP リポジトリを持つ）を id 順にページングして返す
    /// （`com.atproto.sync.listRepos` 用）。
    async fn list_atp_repos(
        &self,
        limit: i64,
        cursor_id: Option<i64>,
    ) -> Result<Vec<Actor>, sqlx::Error>;

    /// アクター ID でアクターを取得する。
    async fn find_by_id(&self, id: i64) -> Result<Option<Actor>, sqlx::Error>;

    /// 複数のアクター ID で一括取得する（DM宛先の種別判定用）。順序は保証しない。
    async fn find_by_ids(&self, ids: &[i64]) -> Result<Vec<Actor>, sqlx::Error>;

    /// ユーザー名 + ドメインから DID のみを取得する（`at_did IS NOT NULL` のもの）。
    async fn find_did_by_username_domain(
        &self,
        username: &str,
        domain: &str,
    ) -> Result<Option<String>, sqlx::Error>;

    /// 既存ユーザー（`user_id`）に紐づく新規ローカルアクターを挿入する。ユーザーと同時に
    /// 作る場合は、両者を1トランザクションで作る`create_local_account`を使う。
    async fn insert_local(
        &self,
        user_id: i64,
        actor: &NewLocalActor<'_>,
    ) -> Result<(), sqlx::Error>;

    /// リモート（Bsky）アクターを upsert し、その actor_id を返す。
    /// `at_did` の一意制約で衝突した場合は handle と display_name を更新する。
    async fn upsert_remote_bsky(
        &self,
        id: i64,
        profile: &BskyActorProfile<'_>,
        now: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error>;

    /// リモート（Fediverse）アクターを upsert し、その actor_id を返す。
    /// `bio`・`avatar_url`・`banner_url` が `None` の場合は既存値を保持する（COALESCE）。
    /// `profile.claimed_at_did` は使わない（相互申告マージは`discover_fedi_actor`の責務）。
    async fn upsert_remote_fedi(
        &self,
        id: i64,
        profile: &FediActorProfile,
        now: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error>;

    /// DID を持つ全ローカルアクターの (username, did) を取得する（起動時 TXT 再登録用）。
    async fn list_local_dids(&self) -> Result<Vec<(String, String)>, sqlx::Error>;

    /// アバター URL を解決する。`avatar_media_id` があれば storage_providers から公開 URL を
    /// 組み立て、なければ `avatar_url`（リモート由来）をそのまま返す。
    async fn find_avatar_url(&self, actor_id: i64) -> Result<Option<String>, sqlx::Error>;

    /// 背景画像（バナー）URL を解決する。`banner_media_id` があれば storage_providers から
    /// 公開 URL を組み立て、なければ `banner_url`（リモート由来）をそのまま返す
    /// （`find_avatar_url` と同じ COALESCE パターン）。
    async fn find_banner_url(&self, actor_id: i64) -> Result<Option<String>, sqlx::Error>;

    /// プロフィール編集用にローカルアクターの現在値を取得する。
    async fn find_profile_by_user_id(
        &self,
        user_id: i64,
    ) -> Result<Option<ActorProfileRow>, sqlx::Error>;

    /// プロフィールを更新する（`update_profile` ハンドラの UPDATE 文）。`emoji_map` は
    /// 呼び出し側が `display_name` から解決済みのショートコード→URLマップ（#186）。
    async fn update_profile(
        &self,
        user_id: i64,
        update: &LocalProfileUpdate<'_>,
    ) -> Result<(), sqlx::Error>;

    /// `actor_id` から生年月日を直接更新する（ATP `putPreferences`
    /// の`#personalDetailsPref`同期用）。公開設定（`birth_date_public`）は変更しない。
    async fn update_birth_date_by_actor_id(
        &self,
        actor_id: i64,
        birth_date: Option<NaiveDate>,
    ) -> Result<(), sqlx::Error>;

    /// `actor_id` から生年月日を取得する（ATP `getPreferences`
    /// の`#personalDetailsPref`生成用）。行が無い/生年月日未設定なら`None`。
    async fn find_birth_date(&self, actor_id: i64) -> Result<Option<NaiveDate>, sqlx::Error>;

    /// 設定画面「プライバシー」から、Bsky Discoverフィード等のアルゴリズムレコメンドから
    /// 除外するよう要求するかどうかを更新する。
    async fn update_hide_from_algorithmic_recommendations(
        &self,
        actor_id: i64,
        hide: bool,
    ) -> Result<(), sqlx::Error>;

    /// 現在の除外設定を取得する。行が無ければ`false`（デフォルト）。
    async fn find_hide_from_algorithmic_recommendations(
        &self,
        actor_id: i64,
    ) -> Result<bool, sqlx::Error>;

    /// フォロー承認制（鍵アカウント）を設定する。
    async fn update_is_locked(&self, actor_id: i64, is_locked: bool) -> Result<(), sqlx::Error>;

    /// 現在の承認制設定を取得する。行が無ければ`false`（デフォルト）。
    async fn find_is_locked(&self, actor_id: i64) -> Result<bool, sqlx::Error>;

    /// ユーザー凍結。`actor_id` はローカル・リモートを問わない（ローカル・リモート共通
    /// enforcementの一本化、通報画面からの凍結・凍結解除、管理画面「凍結済みユーザー」タブが使う）。
    async fn set_suspended(&self, actor_id: i64, suspended: bool) -> Result<(), sqlx::Error>;
}

pub struct PgActorRepository {
    pool: PgPool,
}

impl PgActorRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ActorRepository for PgActorRepository {
    async fn find_local_by_user_id(&self, user_id: i64) -> Result<Option<Actor>, sqlx::Error> {
        sqlx::query_as::<_, Actor>(&format!(
            "SELECT {ACTOR_COLS} FROM actors WHERE user_id = $1 AND actor_type = 'local' LIMIT 1"
        ))
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_by_username_domain(
        &self,
        username: &str,
        domain: &str,
    ) -> Result<Option<Actor>, sqlx::Error> {
        // username は DNS ラベルとして扱う（大文字小文字を区別しない）。
        // `crates/seiran-common/src/username.rs` のモジュールドキュメント参照。
        sqlx::query_as::<_, Actor>(&format!(
            "SELECT {ACTOR_COLS} FROM actors WHERE LOWER(username) = LOWER($1) AND domain = $2 AND withdrawn_at IS NULL LIMIT 1"
        ))
        .bind(username)
        .bind(domain)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_including_withdrawn_by_username_domain(
        &self,
        username: &str,
        domain: &str,
    ) -> Result<Option<Actor>, sqlx::Error> {
        // username は DNS ラベルとして扱う（大文字小文字を区別しない）。
        // `crates/seiran-common/src/username.rs` のモジュールドキュメント参照。
        sqlx::query_as::<_, Actor>(&format!(
            "SELECT {ACTOR_COLS} FROM actors WHERE LOWER(username) = LOWER($1) AND domain = $2 LIMIT 1"
        ))
        .bind(username)
        .bind(domain)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_by_ap_uri(&self, ap_uri: &str) -> Result<Option<Actor>, sqlx::Error> {
        sqlx::query_as::<_, Actor>(&format!(
            "SELECT {ACTOR_COLS} FROM actors WHERE ap_uri = $1 LIMIT 1"
        ))
        .bind(ap_uri)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_by_did(&self, did: &str) -> Result<Option<Actor>, sqlx::Error> {
        sqlx::query_as::<_, Actor>(&format!(
            "SELECT {ACTOR_COLS} FROM actors WHERE at_did = $1 LIMIT 1"
        ))
        .bind(did)
        .fetch_optional(&self.pool)
        .await
    }

    async fn list_atp_repos(
        &self,
        limit: i64,
        cursor_id: Option<i64>,
    ) -> Result<Vec<Actor>, sqlx::Error> {
        sqlx::query_as::<_, Actor>(&format!(
            "SELECT {ACTOR_COLS} FROM actors
             WHERE at_did IS NOT NULL AND ($2::bigint IS NULL OR id > $2)
             ORDER BY id ASC
             LIMIT $1"
        ))
        .bind(limit)
        .bind(cursor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_by_id(&self, id: i64) -> Result<Option<Actor>, sqlx::Error> {
        sqlx::query_as::<_, Actor>(&format!(
            "SELECT {ACTOR_COLS} FROM actors WHERE id = $1 LIMIT 1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_by_ids(&self, ids: &[i64]) -> Result<Vec<Actor>, sqlx::Error> {
        sqlx::query_as::<_, Actor>(&format!(
            "SELECT {ACTOR_COLS} FROM actors WHERE id = ANY($1)"
        ))
        .bind(ids)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_did_by_username_domain(
        &self,
        username: &str,
        domain: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        // resolveHandle / well-known はハンドルを DNS ラベルとして扱うため大文字小文字を
        // 区別しない（`crates/seiran-common/src/username.rs` 参照）。
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT at_did FROM actors
             WHERE LOWER(username) = LOWER($1) AND domain = $2 AND at_did IS NOT NULL LIMIT 1",
        )
        .bind(username)
        .bind(domain)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }

    async fn insert_local(
        &self,
        user_id: i64,
        actor: &NewLocalActor<'_>,
    ) -> Result<(), sqlx::Error> {
        let mut conn = self.pool.acquire().await?;
        insert_local_actor_row(&mut conn, user_id, actor).await
    }

    async fn upsert_remote_bsky(
        &self,
        id: i64,
        profile: &BskyActorProfile<'_>,
        now: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error> {
        // 既に`remote_seiran`へ昇格済み（結婚成立済み、#236）の行に対しては`username`を
        // ATPハンドル形式（`user.pds-domain`）で上書きしない。結婚後の正式なusernameは
        // Fedi側由来のまま保つ（`seiran_actor_merge::discover_bsky_actor`の対称ロジック）。
        // このガードが無いと、フォロワーポーリング等マージロジックを経由しない呼び出し元
        // （`bsky_follower_poll`・`search`等）が定期的に上書きしてしまう（例:
        // `@yubao@beta.seiran.org`のusernameが`yubao.beta.seiran.org`に化ける）。
        // 一方`at_handle`はプロフィール画面のBsky ID表示専用の別列のため、`username`とは
        // 独立に`remote_seiran`でも常に最新値へ更新する。
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO actors (id, actor_type, at_did, username, domain, display_name, avatar_url, banner_url, at_handle, created_at, updated_at)
             VALUES ($1, 'bsky', $2, $3, '', $4, $5, $6, $3, $7, $7)
             ON CONFLICT (at_did) DO UPDATE
               SET username     = CASE WHEN actors.actor_type = 'remote_seiran' THEN actors.username
                                        ELSE EXCLUDED.username END,
                   display_name = COALESCE(EXCLUDED.display_name, actors.display_name),
                   avatar_url   = COALESCE(EXCLUDED.avatar_url, actors.avatar_url),
                   banner_url   = COALESCE(EXCLUDED.banner_url, actors.banner_url),
                   at_handle    = EXCLUDED.at_handle,
                   updated_at   = EXCLUDED.updated_at
             RETURNING id",
        )
        .bind(id)
        .bind(profile.at_did)
        .bind(profile.handle)
        .bind(profile.display_name)
        .bind(profile.avatar_url)
        .bind(profile.banner_url)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    async fn upsert_remote_fedi(
        &self,
        id: i64,
        profile: &FediActorProfile,
        now: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO actors (id, actor_type, ap_uri, ap_inbox_url, username, domain, display_name, avatar_url, banner_url, bio, created_at, updated_at, emoji_map, profile_fields)
             VALUES ($1, 'fedi', $2, $3, $4, $5, $6, $7, $8, $9, $10, $10, $11, $12)
             ON CONFLICT (ap_uri) DO UPDATE
               SET ap_inbox_url   = EXCLUDED.ap_inbox_url,
                   display_name   = EXCLUDED.display_name,
                   avatar_url     = COALESCE(EXCLUDED.avatar_url, actors.avatar_url),
                   banner_url     = COALESCE(EXCLUDED.banner_url, actors.banner_url),
                   bio            = COALESCE(EXCLUDED.bio, actors.bio),
                   emoji_map      = EXCLUDED.emoji_map,
                   profile_fields = EXCLUDED.profile_fields,
                   updated_at     = EXCLUDED.updated_at
             RETURNING id",
        )
        .bind(id)
        .bind(&profile.ap_uri)
        .bind(&profile.ap_inbox_url)
        .bind(&profile.username)
        .bind(&profile.domain)
        .bind(&profile.display_name)
        .bind(&profile.avatar_url)
        .bind(&profile.banner_url)
        .bind(&profile.bio)
        .bind(now)
        .bind(&profile.emoji_map)
        .bind(&profile.profile_fields)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    async fn list_local_dids(&self) -> Result<Vec<(String, String)>, sqlx::Error> {
        sqlx::query_as::<_, (String, String)>(
            "SELECT username, at_did FROM actors
             WHERE actor_type = 'local' AND at_did IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await
    }

    async fn find_avatar_url(&self, actor_id: i64) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) \
             FROM actors a \
             LEFT JOIN media_files mf ON mf.id = a.avatar_media_id \
             LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
             WHERE a.id = $1",
        )
        .bind(actor_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|(url,)| url))
    }

    async fn find_banner_url(&self, actor_id: i64) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.banner_url) \
             FROM actors a \
             LEFT JOIN media_files mf ON mf.id = a.banner_media_id \
             LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
             WHERE a.id = $1",
        )
        .bind(actor_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|(url,)| url))
    }

    async fn find_profile_by_user_id(
        &self,
        user_id: i64,
    ) -> Result<Option<ActorProfileRow>, sqlx::Error> {
        sqlx::query_as::<_, ActorProfileRow>(
            "SELECT id, username, display_name, bio, avatar_media_id, banner_media_id, \
                    profile_fields, emoji_map, birth_date, birth_date_public \
             FROM actors WHERE user_id = $1 AND actor_type = 'local' LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn update_profile(
        &self,
        user_id: i64,
        update: &LocalProfileUpdate<'_>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE actors \
             SET display_name = $1, bio = $2, avatar_media_id = $3, banner_media_id = $4, \
                 profile_fields = $5, emoji_map = $6, birth_date = $7, birth_date_public = $8, \
                 updated_at = NOW() \
             WHERE user_id = $9 AND actor_type = 'local'",
        )
        .bind(update.display_name)
        .bind(update.bio)
        .bind(update.avatar_media_id)
        .bind(update.banner_media_id)
        .bind(update.profile_fields)
        .bind(update.emoji_map)
        .bind(update.birth_date)
        .bind(update.birth_date_public)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn update_birth_date_by_actor_id(
        &self,
        actor_id: i64,
        birth_date: Option<NaiveDate>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE actors SET birth_date = $1, updated_at = NOW() WHERE id = $2")
            .bind(birth_date)
            .bind(actor_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn find_birth_date(&self, actor_id: i64) -> Result<Option<NaiveDate>, sqlx::Error> {
        sqlx::query_scalar::<_, Option<NaiveDate>>("SELECT birth_date FROM actors WHERE id = $1")
            .bind(actor_id)
            .fetch_optional(&self.pool)
            .await
            .map(|r| r.flatten())
    }

    async fn update_hide_from_algorithmic_recommendations(
        &self,
        actor_id: i64,
        hide: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE actors SET hide_from_algorithmic_recommendations = $1, updated_at = NOW() WHERE id = $2",
        )
        .bind(hide)
        .bind(actor_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn find_hide_from_algorithmic_recommendations(
        &self,
        actor_id: i64,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, bool>(
            "SELECT hide_from_algorithmic_recommendations FROM actors WHERE id = $1",
        )
        .bind(actor_id)
        .fetch_optional(&self.pool)
        .await
        .map(|r| r.unwrap_or(false))
    }

    async fn update_is_locked(&self, actor_id: i64, is_locked: bool) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE actors SET is_locked = $1, updated_at = NOW() WHERE id = $2")
            .bind(is_locked)
            .bind(actor_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn find_is_locked(&self, actor_id: i64) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, bool>("SELECT is_locked FROM actors WHERE id = $1")
            .bind(actor_id)
            .fetch_optional(&self.pool)
            .await
            .map(|r| r.unwrap_or(false))
    }

    async fn set_suspended(&self, actor_id: i64, suspended: bool) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE actors SET suspended_at = CASE WHEN $1 THEN NOW() ELSE NULL END, updated_at = NOW() WHERE id = $2",
        )
        .bind(suspended)
        .bind(actor_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }
}

/// アクターのアバター・バナーの解決済み URL と非正規化カウンタ（Misskey 互換 `UserDetailed`）。
#[derive(Debug, sqlx::FromRow)]
pub struct ActorMediaCountsRow {
    pub id: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub banner_url: Option<String>,
    pub notes_count: i64,
    pub followers_count: i64,
    pub following_count: i64,
}

pub async fn media_and_counts_for_actors(
    pool: &sqlx::PgPool,
    ids: &[i64],
) -> Result<Vec<ActorMediaCountsRow>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT a.id, a.created_at, a.display_name, \
         COALESCE(rtrim(avatar_sp.public_url, '/') || '/' || avatar_mf.storage_key, a.avatar_url) AS avatar_url, \
         COALESCE(rtrim(banner_sp.public_url, '/') || '/' || banner_mf.storage_key, a.banner_url) AS banner_url, \
         a.notes_count, a.followers_count, a.following_count \
         FROM actors a \
         LEFT JOIN media_files avatar_mf ON avatar_mf.id = a.avatar_media_id \
         LEFT JOIN storage_providers avatar_sp ON avatar_sp.id = avatar_mf.storage_provider_id \
         LEFT JOIN media_files banner_mf ON banner_mf.id = a.banner_media_id \
         LEFT JOIN storage_providers banner_sp ON banner_sp.id = banner_mf.storage_provider_id \
         WHERE a.id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
}

/// 一覧表示用のアクターの最小情報（解決済みアバター URL 付き）。
#[derive(Debug, sqlx::FromRow)]
pub struct ActorLiteRow {
    pub id: i64,
    pub username: String,
    pub domain: String,
    pub actor_type: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub emoji_map: Option<serde_json::Value>,
}

pub async fn lite_rows_for_actors(
    pool: &sqlx::PgPool,
    ids: &[i64],
) -> Result<Vec<ActorLiteRow>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT a.id, a.username, a.domain, a.actor_type::text AS actor_type, a.display_name, \
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url, \
                a.emoji_map \
         FROM actors a \
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id \
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
         WHERE a.id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
}

/// DID 転出済みにする（既に設定済みなら最初の時刻を保つ）。
pub async fn mark_did_moved_out(
    pool: &sqlx::PgPool,
    actor_id: i64,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE actors SET did_moved_out_at = COALESCE(did_moved_out_at, $1) WHERE id = $2")
        .bind(at)
        .bind(actor_id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// DID 転入で、転入元 DID のリモートキャッシュ行をローカルアクターに変換する。
pub struct ConvertToLocal<'a> {
    pub actor_id: i64,
    pub user_id: i64,
    pub username: &'a str,
    pub domain: &'a str,
    pub ap_uri: &'a str,
    pub signing_key_pem: &'a str,
    pub rotation_key_pem: &'a str,
}

pub async fn convert_remote_to_local(
    pool: &sqlx::PgPool,
    c: &ConvertToLocal<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE actors SET actor_type = 'local', user_id = $1, username = $2, domain = $3,
             ap_uri = $4, at_signing_key_pem = $5, at_rotation_key_pem = $6, updated_at = NOW()
         WHERE id = $7",
    )
    .bind(c.user_id)
    .bind(c.username)
    .bind(c.domain)
    .bind(c.ap_uri)
    .bind(c.signing_key_pem)
    .bind(c.rotation_key_pem)
    .bind(c.actor_id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// OGP に使う、未退会アクターのプロフィール。
#[derive(Debug, sqlx::FromRow)]
pub struct OgpActorRow {
    pub actor_id: i64,
    pub actor_type: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
}

pub async fn ogp_profile(
    pool: &sqlx::PgPool,
    username: &str,
    domain: &str,
) -> Result<Option<OgpActorRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id AS actor_id, a.actor_type::text AS actor_type, a.display_name, a.bio, \
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url \
         FROM actors a \
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id \
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
         WHERE a.username = $1 AND a.domain = $2 AND a.withdrawn_at IS NULL LIMIT 1",
    )
    .bind(username)
    .bind(domain)
    .fetch_optional(pool)
    .await
}

/// ATP プロフィールの再コミットに必要な、表示名・自己紹介・プロフィール項目とアバター・
/// バナーの blob 情報（`sha256`・`mime_type`・`size`）。
#[derive(Debug, sqlx::FromRow)]
pub struct AtpProfileMaterialRow {
    pub username: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub profile_fields: serde_json::Value,
    pub avatar_sha256: Option<String>,
    pub avatar_mime_type: Option<String>,
    pub avatar_size: Option<i64>,
    pub banner_sha256: Option<String>,
    pub banner_mime_type: Option<String>,
    pub banner_size: Option<i64>,
}

pub async fn atp_profile_material(
    pool: &sqlx::PgPool,
    actor_id: i64,
) -> Result<AtpProfileMaterialRow, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.username, a.display_name, a.bio, a.profile_fields, \
                avatar_mf.sha256 AS avatar_sha256, avatar_mf.mime_type AS avatar_mime_type, avatar_mf.size AS avatar_size, \
                banner_mf.sha256 AS banner_sha256, banner_mf.mime_type AS banner_mime_type, banner_mf.size AS banner_size \
         FROM actors a
         LEFT JOIN media_files avatar_mf ON avatar_mf.id = a.avatar_media_id
         LEFT JOIN media_files banner_mf ON banner_mf.id = a.banner_media_id
         WHERE a.id = $1",
    )
    .bind(actor_id)
    .fetch_one(pool)
    .await
}

/// 凍結済みアクター（管理画面）。
#[derive(Debug, sqlx::FromRow)]
pub struct SuspendedActorRow {
    pub id: i64,
    pub username: String,
    pub domain: String,
    pub actor_type: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub suspended_at: chrono::DateTime<chrono::Utc>,
    /// ローカルアクターの場合のみ `Some`。
    pub user_id: Option<i64>,
    pub email: Option<String>,
}

/// `after_id` より後を id 順に最大 `limit` 件。
pub async fn list_suspended(
    pool: &sqlx::PgPool,
    after_id: Option<i64>,
    limit: i64,
) -> Result<Vec<SuspendedActorRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.username, a.domain, a.actor_type::text AS actor_type, a.display_name,
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url,
                a.suspended_at, a.user_id, u.email
         FROM actors a
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         LEFT JOIN users u ON u.id = a.user_id
         WHERE a.suspended_at IS NOT NULL AND ($1::bigint IS NULL OR a.id > $1)
         ORDER BY a.id
         LIMIT $2",
    )
    .bind(after_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn set_bridge_real_actor(
    pool: &PgPool,
    actor_id: i64,
    real_actor_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE actors SET bridge_real_actor_id = $1 WHERE id = $2")
        .bind(real_actor_id)
        .bind(actor_id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Bsky フォロワー検知の対象（ATP 署名鍵を持つ在籍ローカルユーザー）の `(id, at_did)`。
pub async fn bsky_follower_poll_targets(pool: &PgPool) -> Result<Vec<(i64, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, at_did FROM actors
         WHERE actor_type = 'local' AND at_did IS NOT NULL AND at_signing_key_pem IS NOT NULL
           AND withdrawn_at IS NULL",
    )
    .fetch_all(pool)
    .await
}

pub async fn bsky_followers_baseline_done(
    pool: &PgPool,
    actor_id: i64,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT bsky_followers_baseline_done_at IS NOT NULL FROM actors WHERE id = $1",
    )
    .bind(actor_id)
    .fetch_one(pool)
    .await
}

pub async fn mark_bsky_followers_baseline_done(
    pool: &PgPool,
    actor_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE actors SET bsky_followers_baseline_done_at = NOW() WHERE id = $1")
        .bind(actor_id)
        .execute(pool)
        .await
        .map(|_| ())
}

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// リプライ/引用/リポストのどの参照を指すか（#230/#233）。投稿詳細取得時の同期フェッチ・
/// 手動「取り込む」APIエンドポイントで、対象列を選ぶために使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    Reply,
    Quote,
    Repost,
}

impl ReferenceKind {
    /// (post_idカラム名, ap_uriカラム名, ref_statusカラム名) のリテラル3つ組。
    /// バリアントは3値のみでユーザー入力を含まないため、SQL文字列への直接埋め込みでも安全。
    fn columns(self) -> (&'static str, &'static str, &'static str) {
        match self {
            ReferenceKind::Reply => ("reply_to_post_id", "reply_to_ap_uri", "reply_to_ref_status"),
            ReferenceKind::Quote => ("quote_of_post_id", "quote_of_ap_uri", "quote_of_ref_status"),
            ReferenceKind::Repost => (
                "repost_of_post_id",
                "repost_of_ap_uri",
                "repost_of_ref_status",
            ),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ReferenceKind::Reply => "reply",
            ReferenceKind::Quote => "quote",
            ReferenceKind::Repost => "repost",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "reply" => Some(ReferenceKind::Reply),
            "quote" => Some(ReferenceKind::Quote),
            "repost" => Some(ReferenceKind::Repost),
            _ => None,
        }
    }
}

/// タイムライン表示用のポスト + アクター結合行。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TimelinePost {
    pub id: i64,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub actor_id: i64,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    // 7.2 拡張フィールド（古いクエリとの互換のため #[sqlx(default)] を付与）
    #[sqlx(default)]
    pub actor_type: String,
    #[sqlx(default)]
    pub repost_of_post_id: Option<i64>,
    #[sqlx(default)]
    pub quote_of_post_id: Option<i64>,
    #[sqlx(default)]
    pub reply_to_post_id: Option<i64>,
    #[sqlx(default)]
    pub parent_original_post_id: Option<i64>,
    /// 投稿者アバター URL（local は avatar_media_id 解決、remote は actors.avatar_url）。
    #[sqlx(default)]
    pub avatar_url: Option<String>,
    /// 投稿本文中のカスタム絵文字（`:shortcode:`）→画像URLマップ（Fedi受信、AP `tag` 配列由来）。
    #[sqlx(default)]
    pub post_emoji_map: Option<serde_json::Value>,
    /// 投稿者アクターの表示名中のカスタム絵文字→画像URLマップ。
    #[sqlx(default)]
    pub actor_emoji_map: Option<serde_json::Value>,
    /// 可視性（`public`/`unlisted`/`followers_only`/`direct`）。Fedi受信ポストは`to`/`cc`から
    /// 判定した値、ローカル投稿は常に`public`（可視性選択は将来課題）。
    #[sqlx(default)]
    pub visibility: String,
    /// ローカル投稿が実際にFedi/Bskyへ配送されたか（投稿作成時の`deliver_to_fedi`/`deliver_to_bsky`
    /// を永続化したもの）。ローカル投稿以外では意味を持たない。
    #[sqlx(default)]
    pub deliver_fedi: bool,
    #[sqlx(default)]
    pub deliver_bsky: bool,
    /// Bsky メンションfacetの位置情報（`[{"byteStart":N,"byteEnd":M,"did":"did:plc:..."}]`）。
    /// `body` 自体は書き換えず、表示時（`to_note_response`）に都度 DID を解決して
    /// `@handle.domain` へ置換する（ハンドルは可変なため）。ローカル投稿・Fedi受信は常に空配列。
    #[sqlx(default)]
    pub mention_facets: Option<serde_json::Value>,
    /// リモート投稿の AP Note ID（`posts.ap_object_id`）。「リモートで表示」リンク組み立て用
    /// （ローカル投稿・Bsky受信投稿では `None`）。全クエリで取得しているわけではない。
    #[sqlx(default)]
    pub post_ap_object_id: Option<String>,
    /// リモート投稿の AT URI（`posts.at_uri`、`at://did/collection/rkey` 形式）。
    /// 「リモートで表示」リンク組み立て用（ローカル投稿・Fedi受信投稿では `None`）。
    #[sqlx(default)]
    pub post_at_uri: Option<String>,
    #[sqlx(default)]
    pub content_warning: Option<String>,
    #[sqlx(default)]
    pub poll: Option<serde_json::Value>,
    /// このポストへの返信・引用・リポストの件数（`posts.reply_count`/`quote_count`/`repost_count`、
    /// トリガー `posts_apply_relation_counts` により INSERT/論理削除時に自動増減する非正規化カウンタ）。
    #[sqlx(default)]
    pub reply_count: i64,
    #[sqlx(default)]
    pub quote_count: i64,
    #[sqlx(default)]
    pub repost_count: i64,
    /// サニタイズ済みHTML（seiran Web UIでのリッチ表示用、#233）。リモートFedi投稿のみ
    /// 設定。ローカル投稿・Bsky投稿・移行前の既存行は`None`（フロントは`body`の
    /// プレーンテキスト描画にフォールバックする）。
    #[sqlx(default)]
    pub content_html: Option<String>,
    /// 参照が未解決の場合の生AP URIと状態（`"pending"`/`"gone"`、#230）。
    /// 対応する`*_post_id`がSomeなら意味を持たない。現状は投稿詳細取得（`find_by_id`/
    /// `find_by_id_for_viewer`）のみ取得する（他のタイムラインクエリでは常に`None`）。
    #[sqlx(default)]
    pub reply_to_ap_uri: Option<String>,
    #[sqlx(default)]
    pub reply_to_ref_status: Option<String>,
    #[sqlx(default)]
    pub quote_of_ap_uri: Option<String>,
    #[sqlx(default)]
    pub quote_of_ref_status: Option<String>,
    #[sqlx(default)]
    pub repost_of_ap_uri: Option<String>,
    #[sqlx(default)]
    pub repost_of_ref_status: Option<String>,
}

/// プロフィール表示用のポスト要約。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PostSummary {
    pub id: i64,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

/// XRPC getRecord / listRecords 用のレコード行。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PostRecord {
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub at_uri: String,
    pub at_cid: String,
    pub at_rkey: String,
}

/// リポスト・リプライ・引用の配送先を判定するために必要な、元ポストのメタ情報。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PostDeliveryMeta {
    /// 元ポストの投稿者actor_id（ブロック関係チェック用）。
    pub actor_id: i64,
    pub ap_object_id: Option<String>,
    pub at_uri: Option<String>,
    pub at_cid: Option<String>,
    pub domain: String,
    /// `actors.actor_type`（`"local"`/`"fedi"`/`"bsky"`等）。ローカル/リモート判定は
    /// `domain == local_domain`の文字列比較ではなくこちらを使う
    /// （`insert_local`の不変条件によりlocal⇔domain=local_domainは常に一致するが、
    /// actor_type判定の方が環境非依存でSQL側の意味とも一致する）。
    pub actor_type: String,
    pub display_name: Option<String>,
    pub username: String,
    pub body: String,
    pub avatar_url: Option<String>,
    pub first_image_url: Option<String>,
    /// 元ポストの可視性（"public"|"unlisted"|"followers_only"|"direct"）。
    /// リポスト可否判定・可視性継承・Bsky配送許可判定に使う。
    pub visibility: String,
    /// 元ポストが`direct`の場合のスレッド起点ポストID。DM返信時、この値を子ポストへ
    /// そのまま伝播コピーする（元ポストが`direct`でなければ`None`）。
    pub thread_root_post_id: Option<i64>,
    /// ローカル投稿が実際にFedi/Bskyへ配送されたか（投稿作成時の`deliver_to_fedi`/
    /// `deliver_to_bsky`を永続化したもの）。ローカル投稿以外では常に`true`固定で意味を持たない
    /// （リモート受信時はこのカラムに触れずDBデフォルトのままのため）。`ap_object_id`はローカル
    /// 投稿なら`deliver_fedi`の値に関わらず常に生成されるため、返信の配送先制御はローカル投稿では
    /// 実体の有無ではなくこのフラグを直接見る必要がある（`notes::delivery::reply_delivery_allowed`）。
    pub deliver_fedi: bool,
    pub deliver_bsky: bool,
}

/// DMメッセージセッション（スレッド起点を同じくするdirect投稿の集合）の要約。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DmSessionSummary {
    pub thread_root_post_id: i64,
    pub last_post_id: i64,
    pub last_body: String,
    pub last_created_at: DateTime<Utc>,
    /// 自分以外の宛先アクターID一覧（グループではないため実務上1件のことが多いが、
    /// 過去参加者を含め複数になりうる）。
    pub peer_actor_ids: Vec<i64>,
}

/// リポスト取り消し（Undo）に必要な情報。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RepostUndoInfo {
    pub repost_id: i64,
    pub repost_ap_id: Option<String>,
    pub orig_ap_id: Option<String>,
    pub atp_repost_rkey: Option<String>,
    /// Fedi リモートポストのリポスト時に作った Bsky フォールバックテキスト投稿の rkey。
    /// `atp_repost_rkey`（ネイティブ ATP repost）とは排他。
    pub at_rkey: Option<String>,
    /// 元ポスト（repost_of_post_id 先）の at_uri。`orig_ap_id` が無くこれがある場合、
    /// 元ポストは Bsky ネイティブであり、Fedi へは Announce ではなく
    /// `PostToFollowers` の Create(Note) フォールバックを送っている。
    pub orig_at_uri: Option<String>,
}

/// リポストタブ（#226）の1件。リポストラッパー自身の投稿行（`repost_of_post_id`が
/// 対象ポストを指す行）から取る。`deleted_at`が非NULLなら取り消し済み（履歴として残す）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RepostEntry {
    /// リポストラッパー自身の投稿ID（詳細画面へのリンク先、`deleted_at`がNULLの場合のみ有効）。
    pub id: i64,
    pub actor_id: i64,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub actor_type: String,
    pub avatar_url: Option<String>,
    pub created_at: DateTime<Utc>,
    /// 取り消し済み（Undo済み）リポストなら`Some`。
    pub deleted_at: Option<DateTime<Utc>>,
}

/// 投稿削除（`DELETE /api/notes/:id`）に必要な情報。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PostDeleteInfo {
    /// 投稿者actor_id（本人以外は削除不可）。
    pub actor_id: i64,
    /// この投稿が実際にFediへ配送されたか（true の場合のみ AP Delete(Note) を送る）。
    pub deliver_fedi: bool,
    pub visibility: String,
    /// Bskyへコミット済みの場合の rkey（未コミットなら None）。
    pub at_rkey: Option<String>,
}

/// `PostRepository::insert_full` の引数一式（`docs/coding_rules.md` 引数肥大化対策）。
pub struct InsertFullParams<'a> {
    pub id: i64,
    pub actor_id: i64,
    pub body: &'a str,
    pub ap_object_id: &'a str,
    pub seiran_post_uuid: &'a str,
    pub reply_to_post_id: Option<i64>,
    pub quote_of_post_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub visibility: &'a str,
    pub deliver_fedi: bool,
    pub deliver_bsky: bool,
    /// `visibility='direct'`（DM）専用。direct以外は`None`を渡すこと。
    pub thread_root_post_id: Option<i64>,
    /// `visibility='direct'`（DM）専用。direct以外は空スライスを渡すこと。
    pub recipient_actor_ids: &'a [i64],
    /// 本文中のカスタム絵文字（`:shortcode:`）→画像URLマップ。ローカル投稿作成時に本文から
    /// 抽出・解決した値を渡す（Fedi受信時と同様、これが空だと本文中のショートコードが
    /// 画像化されない）。無ければ `serde_json::json!({})` を渡すこと。
    pub emoji_map: &'a serde_json::Value,
    /// アンケート（#228）。`{multiple, options:[{name,votes}], endTime}`
    /// （Fedi受信Questionと同じ形、`normalize_ap_poll`参照）。無ければ`None`。
    pub poll: Option<&'a serde_json::Value>,
    /// CW（閲覧注意）ガイド文（#229）。無ければ`None`。
    pub content_warning: Option<&'a str>,
    /// ポストの言語（ISO 639-1、2文字コード）。Bsky配送の`langs`にのみ意味を持つ
    /// （AP配送では使わない）。無ければ`None`（従来通り言語情報なし）。
    pub language: Option<&'a str>,
}

/// `PostRepository::insert_remote_with_dedup` の引数一式（`docs/coding_rules.md` 引数肥大化対策）。
pub struct InsertRemoteWithDedupParams<'a> {
    pub id: i64,
    pub actor_id: i64,
    pub body: &'a str,
    /// サニタイズ済みHTML（seiran Web UIでのリッチ表示用、#233）。元のAP `Note.content`
    /// から構造保持してクレンジングした値。`None`ならフロントは`body`のプレーンテキスト
    /// 描画にフォールバックする。
    pub content_html: Option<&'a str>,
    pub ap_object_id: &'a str,
    pub seiran_uuid: Option<&'a str>,
    pub parent_original_post_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub emoji_map: &'a serde_json::Value,
    pub visibility: &'a str,
    pub reply_to_post_id: Option<i64>,
    /// `reply_to_post_id`が`None`でもリプライ先URIが存在する場合の生AP URI（#230）。
    /// 参照が元から無いなら`None`、解決済みなら（`reply_to_post_id`がSomeなら）意味を持たない。
    pub reply_to_ap_uri: Option<&'a str>,
    /// `reply_to_ap_uri`があるときの状態（`"pending"`/`"gone"`）。DB側で`post_reference_status`にキャストする。
    pub reply_to_ref_status: Option<&'a str>,
    /// AP `quoteUrl`/`_misskey_quote`（またはBsky `app.bsky.embed.record`）から解決した
    /// 引用元投稿のローカルID（#116）。未解決・非引用なら `None`。
    pub quote_of_post_id: Option<i64>,
    /// `quote_of_post_id`が`None`でも引用元URIが存在する場合の生AP URI（#230）。
    pub quote_of_ap_uri: Option<&'a str>,
    /// `quote_of_ap_uri`があるときの状態（`"pending"`/`"gone"`）。
    pub quote_of_ref_status: Option<&'a str>,
    /// `visibility='direct'`（DM）専用。direct以外は`None`を渡すこと。
    pub thread_root_post_id: Option<i64>,
    /// `visibility='direct'`（DM）専用。direct以外は空スライスを渡すこと。
    pub recipient_actor_ids: &'a [i64],
    /// `seiranPost.counterpartPostId`（他seiranサーバー間の投稿マージ、#237）。
    /// `Some`の場合、`insert_remote_with_dedup`は挿入前にこの値を`at_uri`に持つ既存行を
    /// 検索し、その既存行自身の`claimed_ap_object_id`が`ap_object_id`を指し返し、かつ
    /// 投稿者（`actor_id`）が一致する場合のみ新規INSERTせず既存行を更新する
    /// （`docs/protocols.md` 5節の相互一致アルゴリズム）。マージ不成立ならこの値を
    /// `claimed_at_uri`として新規行に保存する。`None`なら従来通りの単純INSERT
    /// （seiranPost非対応の一般的なリモート投稿）。
    pub claimed_at_uri: Option<&'a str>,
}

/// `PostRepository::insert_repost` の引数一式（`docs/coding_rules.md` 引数肥大化対策）。
pub struct InsertRepostParams<'a> {
    pub id: i64,
    pub actor_id: i64,
    pub ap_object_id: &'a str,
    pub repost_of_post_id: Option<i64>,
    /// `repost_of_post_id`が`None`でもリポスト対象URIが存在する場合の生AP URI（#230）。
    pub repost_of_ap_uri: Option<&'a str>,
    /// `repost_of_ap_uri`があるときの状態（`"pending"`/`"gone"`）。DB側で`post_reference_status`にキャストする。
    pub repost_of_ref_status: Option<&'a str>,
    pub created_at: DateTime<Utc>,
    pub visibility: &'a str,
}

#[async_trait]
pub trait PostRepository: Send + Sync {
    /// 新規ポストを挿入する。
    async fn insert(
        &self,
        id: i64,
        actor_id: i64,
        body: &str,
        ap_object_id: &str,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// ホームタイムライン（自分 + フォロー中の accepted アクターの投稿）を取得する。
    /// リプライ投稿は、リプライ先投稿の投稿者も自分がフォロー中（または自分自身）の場合のみ
    /// 含める（`post_reply_target_followed`）。
    /// `exclude_direct=true` の場合、自分が宛先の`direct`投稿も含め`direct`を一切含めない
    /// （DM機能のため、フロントエンドはタイムライン取得時に常にこれを指定する。
    /// Misskey API互換のためデフォルト`false`＝自分が宛先の`direct`は含まれる）。
    async fn home_timeline(
        &self,
        actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
        exclude_direct: bool,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// ローカルタイムライン（ローカルアクターの投稿）を取得する。`viewer_actor_id` は閲覧者の
    /// actor_id（匿名なら `None`）。`unlisted` と `followers_only` は投稿者本人を含めて除外し、
    /// `direct`は投稿者本人または宛先（`post_recipients`）のみ取得できる
    /// （可視性による閲覧制御）。`exclude_direct` は `home_timeline` 参照。
    async fn local_timeline(
        &self,
        viewer_actor_id: Option<i64>,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
        exclude_direct: bool,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// ソーシャルタイムライン（自分 + フォロー中 + ローカル全アクターの投稿、リプライ含む、#78）を取得する。
    /// `home_timeline`（自分+フォロー中のみ）と`local_timeline`（ローカル全体のみ）を合成した形。
    /// フォロー中経由（自分+フォロー中）のパートは`home_timeline`と同じリプライ先フォロー条件を
    /// 適用するが、ローカル全体パートはフォロー云々に関係なく無条件で含めるため対象外
    /// （リプライ先フォロー条件は付けない）。
    async fn social_timeline(
        &self,
        actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
        exclude_direct: bool,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// グローバルタイムライン（`posts`テーブルに入ってきた全投稿、リプライ含む、#78）を取得する。
    /// `local_timeline`から`is_local`条件を外したもの。
    async fn global_timeline(
        &self,
        viewer_actor_id: Option<i64>,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
        exclude_direct: bool,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// 指定アクターの最近の投稿を取得する（プロフィール要約用の軽量版）。
    async fn recent_by_actor(
        &self,
        actor_id: i64,
        limit: i64,
    ) -> Result<Vec<PostSummary>, sqlx::Error>;

    /// 指定アクターの最近の投稿を、タイムラインと同じ結合行（アクター情報込み）で取得する。
    /// プロフィール画面でタイムラインと同一の NoteCard を描画するために使う（#43）。
    /// `until_id`/`since_id` は他のタイムライン系クエリと同じカーソルページネーション規約
    /// （プロフィール投稿一覧の無限スクロール用、#64）。
    /// `viewer_actor_id` は閲覧者の actor_id（匿名なら `None`）。`followers_only`/`direct` は
    /// 投稿者本人または accepted フォロワーのみ取得できる（可視性による閲覧制御）。`unlisted` は
    /// プロフィール表示のため除外しない。
    async fn timeline_by_actor(
        &self,
        actor_id: i64,
        viewer_actor_id: Option<i64>,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
        exclude_direct: bool,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// DID + rkey で app.bsky.feed.post レコードを取得する。
    async fn find_record(&self, did: &str, rkey: &str) -> Result<Option<PostRecord>, sqlx::Error>;

    /// 指定アクターの app.bsky.feed.post レコード一覧を at_rkey 順にページングして返す
    /// （`com.atproto.repo.listRecords` 用）。`reverse=false` は rkey 昇順（古い順）、
    /// `true` は降順（新しい順）。`cursor_rkey` は前回レスポンス最後の rkey。
    async fn list_records_by_actor(
        &self,
        actor_id: i64,
        limit: i64,
        cursor_rkey: Option<&str>,
        reverse: bool,
    ) -> Result<Vec<PostRecord>, sqlx::Error>;

    /// ID でポストとアクター情報を取得する（可視性チェック無し、内部整合性チェック専用）。
    /// HTTP公開エンドポイントからは呼ばないこと（`find_by_id_for_viewer` を使う）。
    async fn find_by_id(&self, id: i64) -> Result<Option<TimelinePost>, sqlx::Error>;

    /// `find_by_id` と同じだが `deleted_at` を問わず取得する（論理削除済みでも返す）。
    /// リポストラッパー投稿が取り消し済み（`delete_repost`で論理削除）でも、その
    /// リポストに対して過去に発生した通知自体はDBに残り続けるため、通知一覧構築時に
    /// 元のリポスト情報（`repost_of_post_id`経由で`embed_renotes`が解決する元投稿）を
    /// 復元する用途専用（`misskey::convert::build_notifications`）。通常の表示・操作では
    /// `find_by_id`/`find_by_id_for_viewer`を使うこと。
    async fn find_by_id_including_deleted(
        &self,
        id: i64,
    ) -> Result<Option<TimelinePost>, sqlx::Error>;

    /// 複数IDでポストとアクター情報を一括取得する（可視性チェック無し）。呼び出し元が
    /// 別途アクセス制御を済ませている場合のみ使うこと（DMセッション一覧の最終メッセージ取得等）。
    async fn find_by_ids(&self, ids: &[i64]) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// ID でポストとアクター情報を取得する（閲覧者の可視性チェック付き、HTTP公開エンドポイント専用）。
    /// `followers_only`/`direct` は投稿者本人または accepted フォロワーの `viewer_actor_id` のみ
    /// 取得できる。それ以外（匿名・非フォロワー）には `None` を返す（＝404 相当）。
    /// `unlisted`/`public` は無条件で返す。
    async fn find_by_id_for_viewer(
        &self,
        id: i64,
        viewer_actor_id: Option<i64>,
    ) -> Result<Option<TimelinePost>, sqlx::Error>;

    /// リモートノートを重複無視で挿入する（ON CONFLICT DO NOTHING）。
    async fn insert_remote(
        &self,
        id: i64,
        actor_id: i64,
        body: &str,
        ap_object_id: &str,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// 指定ノートIDより前（id < note_id）の投稿を降順で取得する。`viewer_actor_id` は閲覧者の
    /// actor_id（匿名なら `None`）。`followers_only`/`direct` は投稿者本人または accepted
    /// フォロワーのみ取得できる。
    async fn context_before(
        &self,
        actor_id: i64,
        note_id: i64,
        limit: i64,
        viewer_actor_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// 指定ノートIDより後（id > note_id）の投稿を昇順で取得する。`viewer_actor_id` は
    /// `context_before` と同様。
    async fn context_after(
        &self,
        actor_id: i64,
        note_id: i64,
        limit: i64,
        viewer_actor_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// 指定ノートへの直系リプライ・引用を再帰的に取得する（#226 返信タブ）。
    /// `reply_to_post_id`/`quote_of_post_id` のいずれかが親を指す投稿を子として辿る
    /// `WITH RECURSIVE`。深さ20・件数`limit`で打ち切る。呼び出し側で親子関係から
    /// ツリーを再構築する（`reply_id`/`quote_id` を辿る）。
    async fn thread_descendants(
        &self,
        note_id: i64,
        limit: i64,
        viewer_actor_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// 対象ポストへのリポスト一覧を取得する（#226 リポストタブ）。取り消し済み
    /// （`deleted_at`非NULL）も含めて履歴として返す。新しい順、`limit`件まで。
    async fn reposts_of(&self, post_id: i64, limit: i64) -> Result<Vec<RepostEntry>, sqlx::Error>;

    /// リポスト・リプライ・引用の配送先判定に使う、元ポストのメタ情報を取得する。
    async fn find_delivery_meta(&self, id: i64) -> Result<Option<PostDeliveryMeta>, sqlx::Error>;

    /// `seiran_post_uuid` / リプライ / 引用を含むローカル投稿を挿入する。
    /// `deliver_fedi`/`deliver_bsky` は投稿作成時に実際に配送対象とした値をそのまま永続化する
    /// （タイムライン表示時の配送先アイコン用、#配送先・可視性アイコン追加）。
    /// `thread_root_post_id`/`recipient_actor_ids` は`visibility='direct'`（DM）専用
    /// （direct以外は`None`/空スライスを渡すこと）。`recipient_actor_ids`は同一トランザクション内で
    /// `post_recipients` へも一括挿入する。
    async fn insert_full(&self, params: InsertFullParams<'_>) -> Result<(), sqlx::Error>;

    /// `mention_facets`（Bsky流入DIDメンションの位置情報、`[{"byteStart","byteEnd","did"}, ...]`）
    /// を更新する。ATPレコード起点の投稿作成（`com.atproto.repo.createRecord`等）専用。
    /// `insert_full` は本文からのメンション抽出（`@username`）のみでBsky facetsを扱わないため、
    /// facets由来のDIDメンションは作成後に別途この関数で反映する。
    async fn update_mention_facets(
        &self,
        post_id: i64,
        mention_facets: &serde_json::Value,
    ) -> Result<(), sqlx::Error>;

    /// リポストレコードを挿入する（`ap_object_id`の重複はDO NOTHINGで無視する。#231で
    /// リポスト対象が未解決でも箱行だけは必ず保存する設計にしたため、Announce再配送時の
    /// 冪等性を`UNIQUE(actor_id, repost_of_post_id)`だけに頼れなくなった）。
    /// `repost_of_post_id`が`None`の場合、`repost_of_ap_uri`/`repost_of_ref_status`
    /// （`"pending"`/`"gone"`、DB側で`post_reference_status`にキャスト）に未解決状態を記録する（#230）。
    /// `visibility` は元ポストから継承した値（"public"|"unlisted"）。呼び出し元が
    /// `followers_only`/`direct` を渡さないことを保証する（`create_repost` で事前に禁止）。
    async fn insert_repost(&self, params: InsertRepostParams<'_>) -> Result<(), sqlx::Error>;

    /// `pending`な参照が後から解決された（`resolved_post_id`）、または`gone`と新たに確定した
    /// （`ref_status`）場合に、該当行の`*_post_id`/`*_ref_status`を更新する（#233）。
    /// `resolved_post_id`が`Some`なら`*_post_id`を、`None`なら`ref_status`（`"gone"`想定）を
    /// 更新する。両方`None`の呼び出しは何もしない。
    async fn apply_reference_resolution(
        &self,
        post_id: i64,
        kind: ReferenceKind,
        resolved_post_id: Option<i64>,
        ref_status: Option<&str>,
    ) -> Result<(), sqlx::Error>;

    /// リポストレコードを挿入する（Bsky Jetstream `app.bsky.feed.repost` 受信時、`insert_repost`の
    /// ATP版）。`at_uri`はリポストレコード自体のURI（`at://{did}/app.bsky.feed.repost/{rkey}`）。
    /// Bskyのリポストに可視性の概念は無いため常に`public`固定。
    async fn insert_repost_bsky(
        &self,
        id: i64,
        actor_id: i64,
        at_uri: &str,
        repost_of_post_id: i64,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// 添付ファイルを投稿に紐付ける（ローカルアップロード済みの `media_file_id` を持つケース）。
    async fn attach_media(
        &self,
        post_id: i64,
        media_file_id: i64,
        position: i16,
    ) -> Result<(), sqlx::Error>;

    /// リモート添付 URL を投稿に紐付ける（`media_file_id` を持たない受信投稿用）。
    /// `mime_type` は AP attachment の `mediaType`（判別できなければ推定値）。
    /// `thumbnail_url` は Bsky の動画添付（`app.bsky.embed.video`）等、本体 URL とは別に
    /// サムネイル URL を持つケース向け（無ければ `None`）。
    /// `is_gif` は GIF アニメ由来（Tenor/Klipy、または `presentation:"gif"`）で、
    /// フロントが自動再生・ミュート・ループ・コントロール無し表示に切り替える。
    #[allow(clippy::too_many_arguments)]
    async fn attach_remote_media_url(
        &self,
        post_id: i64,
        url: &str,
        mime_type: Option<&str>,
        thumbnail_url: Option<&str>,
        is_sensitive: bool,
        is_gif: bool,
        position: i16,
    ) -> Result<(), sqlx::Error>;

    /// 指定アクターが `note_id` に対して行ったリポストの取り消しに必要な情報を取得する。
    async fn find_repost_undo_info(
        &self,
        actor_id: i64,
        note_id: i64,
    ) -> Result<Option<RepostUndoInfo>, sqlx::Error>;

    /// 投稿削除に必要な情報（所有者チェック・AP/ATP配送要否判定用）を取得する。
    async fn find_delete_info(&self, id: i64) -> Result<Option<PostDeleteInfo>, sqlx::Error>;

    /// 投稿を id で論理削除する。
    async fn soft_delete_by_id(&self, id: i64) -> Result<(), sqlx::Error>;

    /// 投稿を `ap_object_id` で論理削除する（Undo(Announce) 受信時）。返り値は削除行数。
    async fn soft_delete_by_ap_object_id(&self, ap_object_id: &str) -> Result<u64, sqlx::Error>;

    /// `seiran_post_uuid` から (id, ap_object_id) を取得する（seiran 間マージ判定用）。
    async fn find_by_seiran_uuid(
        &self,
        uuid: &str,
    ) -> Result<Option<(i64, Option<String>)>, sqlx::Error>;

    /// `ap_object_id` を更新する（seiran_uuid マージで AP 側が後着した場合）。
    async fn update_ap_object_id(&self, id: i64, ap_object_id: &str) -> Result<(), sqlx::Error>;

    /// `at_uri` から id を取得する（ブリッジ重複検知用）。
    async fn find_id_by_at_uri(&self, at_uri: &str) -> Result<Option<i64>, sqlx::Error>;

    /// `ap_object_id` または `at_uri` から id を取得する（Announce の元ポスト検索用）。
    async fn find_id_by_ap_or_at_uri(&self, uri: &str) -> Result<Option<i64>, sqlx::Error>;

    /// `ap_object_id` から (id, actor_id) を取得する（Like/EmojiReact の対象ポスト特定用）。
    async fn find_id_and_actor_by_ap_object_id(
        &self,
        ap_object_id: &str,
    ) -> Result<Option<(i64, i64)>, sqlx::Error>;

    /// `at_uri` を持つ投稿を論理削除する（ATP `app.bsky.feed.post` の delete commit 受信用）。
    /// 返り値は (id, actor_id)。既に削除済み/該当なしなら None。
    async fn soft_delete_by_at_uri(&self, at_uri: &str) -> Result<Option<(i64, i64)>, sqlx::Error>;

    /// `at_uri` から (id, actor_id) を取得する（ATP `app.bsky.feed.like` の対象ポスト特定用）。
    async fn find_id_and_actor_by_at_uri(
        &self,
        at_uri: &str,
    ) -> Result<Option<(i64, i64)>, sqlx::Error>;

    /// リモートから受信したノートを、重複排除メタ（seiran_uuid・ループバック/ブリッジ紐付け）付きで挿入する。
    /// `ap_object_id` が既存なら何もしない。
    /// `emoji_map` は本文中のカスタム絵文字（`:shortcode:`）→画像URLのマップ（AP `tag` 配列由来、
    /// 無ければ空オブジェクト）。
    /// `reply_to_post_id`はAP `inReplyTo`から解決した親ポストID（Fedi受信投稿にも設定する）。
    /// `thread_root_post_id`/`recipient_actor_ids`は`visibility='direct'`（DM受信）専用
    /// （direct以外は`None`/空スライスを渡すこと）。
    async fn insert_remote_with_dedup(
        &self,
        params: InsertRemoteWithDedupParams<'_>,
    ) -> Result<(), sqlx::Error>;

    /// `seiranPost`相互一致マージ（#237）で、既に別々の行として存在していた
    /// AP起源行とATP起源行を1行へ統合する同期部分（`docs/protocols.md` 5節
    /// 「マージ成立時のクリーンアップ」参照）。呼び出し元がadvisory lock保持中に呼ぶこと。
    ///
    /// `survivor_id`（生き残る行）へ`ap_object_id`/`at_uri`の両方を確定させ、
    /// `doomed_id`（削除予定行）は当該列をNULL化した上で`parent_original_post_id`を
    /// survivorへ張り替え・論理削除する。実際の関連テーブルのFK付け替え・カウンタ調整・
    /// 物理削除は非同期ジョブ（`Job::PostMergeCleanup`）に委譲する。
    async fn finalize_post_merge(
        &self,
        survivor_id: i64,
        doomed_id: i64,
        ap_object_id: &str,
        at_uri: &str,
    ) -> Result<(), sqlx::Error>;

    async fn set_fedi_content_metadata(
        &self,
        post_id: i64,
        content_warning: Option<&str>,
        poll: Option<&serde_json::Value>,
    ) -> Result<(), sqlx::Error>;

    /// 候補`post_id`群のうち、リモートアンケートの生存監視フォールバック
    /// （`Job::PollFetch`）の対象となるものを返す。`poll_update_received=false`
    /// （Update(Question)を一度も受理していない＝未対応実装の可能性）かつ`poll_fetched_at`が
    /// その投稿ごとの`threshold`より古いものが対象。`threshold`は呼び出し側（`handlers::notes::
    /// queries::enqueue_stale_poll_fetches`）が「締切前=直近10分より古いか」「締切後=締切時刻
    /// より古いか（＝締切後まだ一度も取得できていないか）」を投稿ごとに計算して渡す
    /// （締切前に取り逃した票数を締切後も永久に取り戻せなくなるのを防ぐため、一律の
    /// カットオフではなく投稿ごとの`threshold`にしている）。
    async fn find_stale_remote_poll_post_ids(
        &self,
        candidates: &[(i64, DateTime<Utc>)],
    ) -> Result<Vec<i64>, sqlx::Error>;
}

pub struct PgPostRepository {
    pool: PgPool,
}

impl PgPostRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PostRepository for PgPostRepository {
    async fn insert(
        &self,
        id: i64,
        actor_id: i64,
        body: &str,
        ap_object_id: &str,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "WITH inserted AS (
                 INSERT INTO posts (id, actor_id, body, ap_object_id, created_at)
                 VALUES ($1, $2, $3, $4, $5)
                 RETURNING actor_id
             )
             UPDATE actors SET notes_count = notes_count + 1
             WHERE id = (SELECT actor_id FROM inserted)",
        )
        .bind(id)
        .bind(actor_id)
        .bind(body)
        .bind(ap_object_id)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn home_timeline(
        &self,
        actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
        exclude_direct: bool,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        // `actor_id = $1 OR actor_id IN (follows...)` を素朴に `ORDER BY id DESC LIMIT` すると、
        // posts 全体（Bsky Jetstream 経由で無関係なリモート投稿を含め100万行超）を id 降順に
        // スキャンしながら1行ずつフィルタする実行計画になり、フォロー数が少ないユーザーほど
        // 大量の無関係行を読み飛ばすまで終わらない（実測 2.6秒、104万行スキャンして8行採用）。
        // targets（自分+フォロー中）を LATERAL で actor_id ごとに `idx_posts_actor_id` を引かせ、
        // 各アクター最大 limit 件だけ取ってからマージソートする形に書き換えると、
        // 既存インデックスのみで実測 1〜4ms まで改善する。
        sqlx::query_as::<_, TimelinePost>(
            "WITH targets AS (
                 SELECT $1::bigint AS actor_id
                 UNION
                 SELECT target_actor_id FROM follows
                 WHERE follower_actor_id = $1 AND status = 'accepted'
                   AND NOT actor_is_hidden_for_viewer($1, target_actor_id)
             ),
             candidate_ids AS (
                 SELECT p.id
                 FROM targets t
                 CROSS JOIN LATERAL (
                     SELECT id FROM posts p
                     WHERE p.actor_id = t.actor_id AND p.deleted_at IS NULL
                       AND ($2::bigint IS NULL OR p.id < $2)
                       AND ($3::bigint IS NULL OR p.id > $3)
                       AND post_is_visible_to($1, p.actor_id, p.visibility::text, p.id, $5)
                       AND post_reply_target_followed($1, p.reply_to_post_id)
                       AND NOT ((p.repost_of_post_id IS NOT NULL OR p.repost_of_ap_uri IS NOT NULL) AND repost_is_muted_for_viewer($1, p.actor_id))
                     ORDER BY p.id DESC LIMIT $4
                 ) p
                 ORDER BY p.id DESC LIMIT $4
             )
             SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets, p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count, p.content_html,
                    p.reply_to_ap_uri, p.reply_to_ref_status::text AS reply_to_ref_status,
                    p.quote_of_ap_uri, p.quote_of_ref_status::text AS quote_of_ref_status,
                    p.repost_of_ap_uri, p.repost_of_ref_status::text AS repost_of_ref_status
             FROM candidate_ids ci
             JOIN posts p ON p.id = ci.id
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             ORDER BY p.id DESC",
        )
        .bind(actor_id)
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .bind(exclude_direct)
        .fetch_all(&self.pool)
        .await
    }

    async fn local_timeline(
        &self,
        viewer_actor_id: Option<i64>,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
        exclude_direct: bool,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets, p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count, p.content_html,
                    p.reply_to_ap_uri, p.reply_to_ref_status::text AS reply_to_ref_status,
                    p.quote_of_ap_uri, p.quote_of_ref_status::text AS quote_of_ref_status,
                    p.repost_of_ap_uri, p.repost_of_ref_status::text AS repost_of_ref_status
             FROM posts p JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.is_local = true AND p.deleted_at IS NULL
               AND ($2::bigint IS NULL OR p.id < $2)
               AND ($3::bigint IS NULL OR p.id > $3)
               AND p.visibility NOT IN ('unlisted', 'followers_only')
               AND ($1::bigint IS NULL OR p.actor_id = $1 OR NOT actor_is_hidden_for_viewer($1, p.actor_id))
               AND ($1::bigint IS NULL OR NOT ((p.repost_of_post_id IS NOT NULL OR p.repost_of_ap_uri IS NOT NULL) AND repost_is_muted_for_viewer($1, p.actor_id)))
               AND post_is_visible_to($1, p.actor_id, p.visibility::text, p.id, $5)
             ORDER BY p.id DESC LIMIT $4",
        )
        .bind(viewer_actor_id)
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .bind(exclude_direct)
        .fetch_all(&self.pool)
        .await
    }

    async fn social_timeline(
        &self,
        actor_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
        exclude_direct: bool,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        // home_timeline（自分+フォロー中、LATERAL方式）とlocal_timeline（ローカル全体、
        // is_localインデックス使用）の候補IDをUNIONしてから外側で再度LIMITする。
        // 片方のみのLATERAL/インデックス最適化をそのまま活かせる（#78）。
        sqlx::query_as::<_, TimelinePost>(
            "WITH targets AS (
                 SELECT $1::bigint AS actor_id
                 UNION
                 SELECT target_actor_id FROM follows
                 WHERE follower_actor_id = $1 AND status = 'accepted'
                   AND NOT actor_is_hidden_for_viewer($1, target_actor_id)
             ),
             candidate_ids AS (
                 (
                     SELECT p.id
                     FROM targets t
                     CROSS JOIN LATERAL (
                         SELECT id FROM posts p
                         WHERE p.actor_id = t.actor_id AND p.deleted_at IS NULL
                           AND ($2::bigint IS NULL OR p.id < $2)
                           AND ($3::bigint IS NULL OR p.id > $3)
                           AND post_is_visible_to($1, p.actor_id, p.visibility::text, p.id, $5)
                           AND post_reply_target_followed($1, p.reply_to_post_id)
                           AND NOT ((p.repost_of_post_id IS NOT NULL OR p.repost_of_ap_uri IS NOT NULL) AND repost_is_muted_for_viewer($1, p.actor_id))
                         ORDER BY p.id DESC LIMIT $4
                     ) p
                 )

                 UNION

                 (
                     SELECT p.id
                     FROM posts p
                     WHERE p.is_local = true AND p.deleted_at IS NULL
                       AND ($2::bigint IS NULL OR p.id < $2)
                       AND ($3::bigint IS NULL OR p.id > $3)
                       AND (p.visibility != 'unlisted' OR p.actor_id = $1)
                       AND (p.actor_id = $1 OR NOT actor_is_hidden_for_viewer($1, p.actor_id))
                       AND NOT ((p.repost_of_post_id IS NOT NULL OR p.repost_of_ap_uri IS NOT NULL) AND repost_is_muted_for_viewer($1, p.actor_id))
                       AND post_is_visible_to($1, p.actor_id, p.visibility::text, p.id, $5)
                     ORDER BY p.id DESC LIMIT $4
                 )
             )
             SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets, p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count, p.content_html,
                    p.reply_to_ap_uri, p.reply_to_ref_status::text AS reply_to_ref_status,
                    p.quote_of_ap_uri, p.quote_of_ref_status::text AS quote_of_ref_status,
                    p.repost_of_ap_uri, p.repost_of_ref_status::text AS repost_of_ref_status
             FROM candidate_ids ci
             JOIN posts p ON p.id = ci.id
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             ORDER BY p.id DESC LIMIT $4",
        )
        .bind(actor_id)
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .bind(exclude_direct)
        .fetch_all(&self.pool)
        .await
    }

    async fn global_timeline(
        &self,
        viewer_actor_id: Option<i64>,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
        exclude_direct: bool,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        // local_timeline から `is_local = true` 条件のみを外したもの（#78）。
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets, p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count, p.content_html,
                    p.reply_to_ap_uri, p.reply_to_ref_status::text AS reply_to_ref_status,
                    p.quote_of_ap_uri, p.quote_of_ref_status::text AS quote_of_ref_status,
                    p.repost_of_ap_uri, p.repost_of_ref_status::text AS repost_of_ref_status
             FROM posts p JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.deleted_at IS NULL
               AND ($2::bigint IS NULL OR p.id < $2)
               AND ($3::bigint IS NULL OR p.id > $3)
               AND p.visibility NOT IN ('unlisted', 'followers_only')
               AND ($1::bigint IS NULL OR p.actor_id = $1 OR NOT actor_is_hidden_for_viewer($1, p.actor_id))
               AND ($1::bigint IS NULL OR NOT ((p.repost_of_post_id IS NOT NULL OR p.repost_of_ap_uri IS NOT NULL) AND repost_is_muted_for_viewer($1, p.actor_id)))
               AND post_is_visible_to($1, p.actor_id, p.visibility::text, p.id, $5)
             ORDER BY p.id DESC LIMIT $4",
        )
        .bind(viewer_actor_id)
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .bind(exclude_direct)
        .fetch_all(&self.pool)
        .await
    }

    async fn recent_by_actor(
        &self,
        actor_id: i64,
        limit: i64,
    ) -> Result<Vec<PostSummary>, sqlx::Error> {
        sqlx::query_as::<_, PostSummary>(
            "SELECT id, body, created_at FROM posts
             WHERE actor_id = $1 AND deleted_at IS NULL
             ORDER BY id DESC LIMIT $2",
        )
        .bind(actor_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn timeline_by_actor(
        &self,
        actor_id: i64,
        viewer_actor_id: Option<i64>,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
        exclude_direct: bool,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, p.actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets, p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count, p.content_html,
                    p.reply_to_ap_uri, p.reply_to_ref_status::text AS reply_to_ref_status,
                    p.quote_of_ap_uri, p.quote_of_ref_status::text AS quote_of_ref_status,
                    p.repost_of_ap_uri, p.repost_of_ref_status::text AS repost_of_ref_status
             FROM posts p
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.actor_id = $1 AND p.deleted_at IS NULL
               AND ($3::bigint IS NULL OR p.id < $3)
               AND ($4::bigint IS NULL OR p.id > $4)
               AND ($2::bigint IS NULL OR p.actor_id = $2 OR NOT actor_is_hidden_for_viewer($2, $1))
               AND post_is_visible_to($2, p.actor_id, p.visibility::text, p.id, $6)
             ORDER BY p.id DESC
             LIMIT $5",
        )
        .bind(actor_id)
        .bind(viewer_actor_id)
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .bind(exclude_direct)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_record(&self, did: &str, rkey: &str) -> Result<Option<PostRecord>, sqlx::Error> {
        sqlx::query_as::<_, PostRecord>(
            "SELECT p.body, p.created_at, p.at_uri, p.at_cid, p.at_rkey
             FROM posts p
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE a.at_did = $1 AND p.at_rkey = $2 AND p.deleted_at IS NULL
             LIMIT 1",
        )
        .bind(did)
        .bind(rkey)
        .fetch_optional(&self.pool)
        .await
    }

    async fn list_records_by_actor(
        &self,
        actor_id: i64,
        limit: i64,
        cursor_rkey: Option<&str>,
        reverse: bool,
    ) -> Result<Vec<PostRecord>, sqlx::Error> {
        if reverse {
            sqlx::query_as::<_, PostRecord>(
                "SELECT p.body, p.created_at, p.at_uri, p.at_cid, p.at_rkey
                 FROM posts p
                 WHERE p.actor_id = $1 AND p.deleted_at IS NULL AND p.at_rkey IS NOT NULL
                   AND ($3::text IS NULL OR p.at_rkey < $3)
                 ORDER BY p.at_rkey DESC
                 LIMIT $2",
            )
            .bind(actor_id)
            .bind(limit)
            .bind(cursor_rkey)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as::<_, PostRecord>(
                "SELECT p.body, p.created_at, p.at_uri, p.at_cid, p.at_rkey
                 FROM posts p
                 WHERE p.actor_id = $1 AND p.deleted_at IS NULL AND p.at_rkey IS NOT NULL
                   AND ($3::text IS NULL OR p.at_rkey > $3)
                 ORDER BY p.at_rkey ASC
                 LIMIT $2",
            )
            .bind(actor_id)
            .bind(limit)
            .bind(cursor_rkey)
            .fetch_all(&self.pool)
            .await
        }
    }

    async fn find_by_id(&self, id: i64) -> Result<Option<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets, p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count, p.content_html,
                    p.reply_to_ap_uri, p.reply_to_ref_status::text AS reply_to_ref_status,
                    p.quote_of_ap_uri, p.quote_of_ref_status::text AS quote_of_ref_status,
                    p.repost_of_ap_uri, p.repost_of_ref_status::text AS repost_of_ref_status
             FROM posts p JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.id = $1 AND p.deleted_at IS NULL
             LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_by_id_including_deleted(
        &self,
        id: i64,
    ) -> Result<Option<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets, p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count, p.content_html,
                    p.reply_to_ap_uri, p.reply_to_ref_status::text AS reply_to_ref_status,
                    p.quote_of_ap_uri, p.quote_of_ref_status::text AS quote_of_ref_status,
                    p.repost_of_ap_uri, p.repost_of_ref_status::text AS repost_of_ref_status
             FROM posts p JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.id = $1
             LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_by_ids(&self, ids: &[i64]) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets, p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count, p.content_html,
                    p.reply_to_ap_uri, p.reply_to_ref_status::text AS reply_to_ref_status,
                    p.quote_of_ap_uri, p.quote_of_ref_status::text AS quote_of_ref_status,
                    p.repost_of_ap_uri, p.repost_of_ref_status::text AS repost_of_ref_status
             FROM posts p JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.id = ANY($1) AND p.deleted_at IS NULL",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_by_id_for_viewer(
        &self,
        id: i64,
        viewer_actor_id: Option<i64>,
    ) -> Result<Option<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets, p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count, p.content_html,
                    p.ap_object_id AS post_ap_object_id, p.at_uri AS post_at_uri,
                    p.reply_to_ap_uri, p.reply_to_ref_status::text AS reply_to_ref_status,
                    p.quote_of_ap_uri, p.quote_of_ref_status::text AS quote_of_ref_status,
                    p.repost_of_ap_uri, p.repost_of_ref_status::text AS repost_of_ref_status
             FROM posts p JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.id = $1 AND p.deleted_at IS NULL
               AND post_is_visible_to($2, p.actor_id, p.visibility::text, p.id, false)
             LIMIT 1",
        )
        .bind(id)
        .bind(viewer_actor_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn insert_remote(
        &self,
        id: i64,
        actor_id: i64,
        body: &str,
        ap_object_id: &str,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "WITH inserted AS (
                 INSERT INTO posts (id, actor_id, body, ap_object_id, created_at)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (ap_object_id) DO NOTHING
                 RETURNING actor_id
             )
             UPDATE actors SET notes_count = notes_count + 1
             WHERE id = (SELECT actor_id FROM inserted)",
        )
        .bind(id)
        .bind(actor_id)
        .bind(body)
        .bind(ap_object_id)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn context_before(
        &self,
        actor_id: i64,
        note_id: i64,
        limit: i64,
        viewer_actor_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, p.actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets, p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count, p.content_html,
                    p.reply_to_ap_uri, p.reply_to_ref_status::text AS reply_to_ref_status,
                    p.quote_of_ap_uri, p.quote_of_ref_status::text AS quote_of_ref_status,
                    p.repost_of_ap_uri, p.repost_of_ref_status::text AS repost_of_ref_status
             FROM posts p
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.actor_id = $1 AND p.id < $2 AND p.deleted_at IS NULL
               AND ($4::bigint IS NULL OR p.actor_id = $4 OR NOT actor_is_hidden_for_viewer($4, p.actor_id))
               AND post_is_visible_to($4, p.actor_id, p.visibility::text, p.id, false)
             ORDER BY p.id DESC
             LIMIT $3",
        )
        .bind(actor_id)
        .bind(note_id)
        .bind(limit)
        .bind(viewer_actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn context_after(
        &self,
        actor_id: i64,
        note_id: i64,
        limit: i64,
        viewer_actor_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "SELECT p.id, p.body, p.created_at, p.actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets, p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count, p.content_html,
                    p.reply_to_ap_uri, p.reply_to_ref_status::text AS reply_to_ref_status,
                    p.quote_of_ap_uri, p.quote_of_ref_status::text AS quote_of_ref_status,
                    p.repost_of_ap_uri, p.repost_of_ref_status::text AS repost_of_ref_status
             FROM posts p
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.actor_id = $1 AND p.id > $2 AND p.deleted_at IS NULL
               AND ($4::bigint IS NULL OR p.actor_id = $4 OR NOT actor_is_hidden_for_viewer($4, p.actor_id))
               AND post_is_visible_to($4, p.actor_id, p.visibility::text, p.id, false)
             ORDER BY p.id ASC
             LIMIT $3",
        )
        .bind(actor_id)
        .bind(note_id)
        .bind(limit)
        .bind(viewer_actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn thread_descendants(
        &self,
        note_id: i64,
        limit: i64,
        viewer_actor_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        sqlx::query_as::<_, TimelinePost>(
            "WITH RECURSIVE descendants AS (
                 SELECT p.id, 1 AS depth
                 FROM posts p
                 WHERE (p.reply_to_post_id = $1 OR p.quote_of_post_id = $1)
                   AND p.deleted_at IS NULL
                   AND ($3::bigint IS NULL OR p.actor_id = $3 OR NOT actor_is_hidden_for_viewer($3, p.actor_id))
                   AND post_is_visible_to($3, p.actor_id, p.visibility::text, p.id, false)
                 UNION ALL
                 SELECT p.id, d.depth + 1
                 FROM posts p
                 JOIN descendants d ON (p.reply_to_post_id = d.id OR p.quote_of_post_id = d.id)
                 WHERE p.deleted_at IS NULL AND d.depth < 20
                   AND ($3::bigint IS NULL OR p.actor_id = $3 OR NOT actor_is_hidden_for_viewer($3, p.actor_id))
                   AND post_is_visible_to($3, p.actor_id, p.visibility::text, p.id, false)
             )
             SELECT p.id, p.body, p.created_at, p.actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type, p.repost_of_post_id, p.quote_of_post_id, p.reply_to_post_id, p.parent_original_post_id,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.emoji_map AS post_emoji_map, a.emoji_map AS actor_emoji_map,
                    p.visibility::text AS visibility, p.deliver_fedi, p.deliver_bsky, p.mention_facets, p.content_warning, p.poll, p.reply_count, p.quote_count, p.repost_count, p.content_html,
                    p.reply_to_ap_uri, p.reply_to_ref_status::text AS reply_to_ref_status,
                    p.quote_of_ap_uri, p.quote_of_ref_status::text AS quote_of_ref_status,
                    p.repost_of_ap_uri, p.repost_of_ref_status::text AS repost_of_ref_status
             FROM descendants d
             JOIN posts p ON p.id = d.id
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             ORDER BY d.depth, p.id
             LIMIT $2",
        )
        .bind(note_id)
        .bind(limit)
        .bind(viewer_actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn reposts_of(&self, post_id: i64, limit: i64) -> Result<Vec<RepostEntry>, sqlx::Error> {
        sqlx::query_as::<_, RepostEntry>(
            "SELECT p.id, p.actor_id, a.username, a.domain, a.display_name,
                    a.actor_type::text AS actor_type,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    p.created_at, p.deleted_at
             FROM posts p
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.repost_of_post_id = $1
             ORDER BY p.id DESC
             LIMIT $2",
        )
        .bind(post_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_delivery_meta(&self, id: i64) -> Result<Option<PostDeliveryMeta>, sqlx::Error> {
        sqlx::query_as::<_, PostDeliveryMeta>(
            "SELECT p.actor_id, p.ap_object_id, p.at_uri, p.at_cid,
                    a.domain, a.actor_type::text AS actor_type, a.display_name, a.username, p.body,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    (
                        SELECT COALESCE(
                            rtrim(msp.public_url, '/') || '/' || mf.storage_key,
                            pa.remote_url
                        )
                        FROM post_attachments pa
                        LEFT JOIN media_files mf ON mf.id = pa.media_file_id
                        LEFT JOIN storage_providers msp ON msp.id = mf.storage_provider_id
                        WHERE pa.post_id = p.id
                          AND COALESCE(mf.mime_type, pa.remote_mime_type, '') LIKE 'image/%'
                        ORDER BY pa.position
                        LIMIT 1
                    ) AS first_image_url,
                    p.visibility::text AS visibility, p.thread_root_post_id,
                    p.deliver_fedi, p.deliver_bsky
             FROM posts p
             JOIN actors a ON a.id = p.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE p.id = $1 AND p.deleted_at IS NULL
             LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn insert_full(&self, params: InsertFullParams<'_>) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO posts (id, actor_id, body, ap_object_id, seiran_post_uuid, reply_to_post_id, quote_of_post_id, created_at, visibility, deliver_fedi, deliver_bsky, thread_root_post_id, emoji_map, poll, content_warning, language)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::post_visibility_enum, $10, $11, $12, $13, $14, $15, $16)",
        )
        .bind(params.id)
        .bind(params.actor_id)
        .bind(params.body)
        .bind(params.ap_object_id)
        .bind(params.seiran_post_uuid)
        .bind(params.reply_to_post_id)
        .bind(params.quote_of_post_id)
        .bind(params.created_at)
        .bind(params.visibility)
        .bind(params.deliver_fedi)
        .bind(params.deliver_bsky)
        .bind(params.thread_root_post_id)
        .bind(params.emoji_map)
        .bind(params.poll)
        .bind(params.content_warning)
        .bind(params.language)
        .execute(&mut *tx)
        .await?;

        if !params.recipient_actor_ids.is_empty() {
            sqlx::query(
                "INSERT INTO post_recipients (post_id, actor_id) SELECT $1, unnest($2::bigint[])",
            )
            .bind(params.id)
            .bind(params.recipient_actor_ids)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query("UPDATE actors SET notes_count = notes_count + 1 WHERE id = $1")
            .bind(params.actor_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await
    }

    async fn update_mention_facets(
        &self,
        post_id: i64,
        mention_facets: &serde_json::Value,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE posts SET mention_facets = $1 WHERE id = $2")
            .bind(mention_facets)
            .bind(post_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn apply_reference_resolution(
        &self,
        post_id: i64,
        kind: ReferenceKind,
        resolved_post_id: Option<i64>,
        ref_status: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let (post_id_col, _uri_col, status_col) = kind.columns();
        if let Some(resolved) = resolved_post_id {
            let sql = format!("UPDATE posts SET {post_id_col} = $1 WHERE id = $2");
            sqlx::query(&sql)
                .bind(resolved)
                .bind(post_id)
                .execute(&self.pool)
                .await?;
        } else if let Some(status) = ref_status {
            let sql =
                format!("UPDATE posts SET {status_col} = $1::post_reference_status WHERE id = $2");
            sqlx::query(&sql)
                .bind(status)
                .bind(post_id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    async fn insert_repost(&self, params: InsertRepostParams<'_>) -> Result<(), sqlx::Error> {
        sqlx::query(
            "WITH inserted AS (
                 INSERT INTO posts (id, actor_id, body, ap_object_id, repost_of_post_id, created_at, visibility, repost_of_ap_uri, repost_of_ref_status)
                 VALUES ($1, $2, '', $3, $4, $5, $6::post_visibility_enum, $7, $8::post_reference_status)
                 ON CONFLICT (ap_object_id) DO NOTHING
                 RETURNING actor_id
             )
             UPDATE actors SET notes_count = notes_count + 1
             WHERE id = (SELECT actor_id FROM inserted)",
        )
        .bind(params.id)
        .bind(params.actor_id)
        .bind(params.ap_object_id)
        .bind(params.repost_of_post_id)
        .bind(params.created_at)
        .bind(params.visibility)
        .bind(params.repost_of_ap_uri)
        .bind(params.repost_of_ref_status)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn insert_repost_bsky(
        &self,
        id: i64,
        actor_id: i64,
        at_uri: &str,
        repost_of_post_id: i64,
        created_at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "WITH inserted AS (
                 INSERT INTO posts (id, actor_id, body, at_uri, repost_of_post_id, created_at, visibility)
                 VALUES ($1, $2, '', $3, $4, $5, 'public')
                 RETURNING actor_id
             )
             UPDATE actors SET notes_count = notes_count + 1
             WHERE id = (SELECT actor_id FROM inserted)",
        )
        .bind(id)
        .bind(actor_id)
        .bind(at_uri)
        .bind(repost_of_post_id)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn attach_media(
        &self,
        post_id: i64,
        media_file_id: i64,
        position: i16,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO post_attachments (post_id, media_file_id, position) VALUES ($1, $2, $3)",
        )
        .bind(post_id)
        .bind(media_file_id)
        .bind(position)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn attach_remote_media_url(
        &self,
        post_id: i64,
        url: &str,
        mime_type: Option<&str>,
        thumbnail_url: Option<&str>,
        is_sensitive: bool,
        is_gif: bool,
        position: i16,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO post_attachments (post_id, media_file_id, remote_url, remote_mime_type, remote_thumbnail_url, is_sensitive, is_gif, position)
             VALUES ($1, NULL, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (post_id, position) DO NOTHING",
        )
        .bind(post_id)
        .bind(url)
        .bind(mime_type)
        .bind(thumbnail_url)
        .bind(is_sensitive)
        .bind(is_gif)
        .bind(position)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn find_repost_undo_info(
        &self,
        actor_id: i64,
        note_id: i64,
    ) -> Result<Option<RepostUndoInfo>, sqlx::Error> {
        sqlx::query_as::<_, RepostUndoInfo>(
            "SELECT p.id AS repost_id, p.ap_object_id AS repost_ap_id,
                    p.atp_repost_rkey, p.at_rkey,
                    orig.ap_object_id AS orig_ap_id, orig.at_uri AS orig_at_uri
             FROM posts p
             JOIN posts orig ON orig.id = p.repost_of_post_id
             WHERE p.actor_id = $1 AND p.repost_of_post_id = $2 AND p.deleted_at IS NULL
             LIMIT 1",
        )
        .bind(actor_id)
        .bind(note_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_delete_info(&self, id: i64) -> Result<Option<PostDeleteInfo>, sqlx::Error> {
        sqlx::query_as::<_, PostDeleteInfo>(
            "SELECT actor_id, deliver_fedi, visibility::text AS visibility, at_rkey
             FROM posts
             WHERE id = $1 AND deleted_at IS NULL
             LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn soft_delete_by_id(&self, id: i64) -> Result<(), sqlx::Error> {
        // `deleted_at IS NULL` ガードで「実際に未削除→削除済みへ遷移したか」を判定し、
        // 既に削除済みの行への重複呼び出しでnotes_countを二重に減らさないようにする。
        sqlx::query(
            "WITH deleted AS (
                 UPDATE posts SET deleted_at = NOW() WHERE id = $1 AND deleted_at IS NULL
                 RETURNING actor_id
             )
             UPDATE actors SET notes_count = GREATEST(notes_count - 1, 0)
             WHERE id = (SELECT actor_id FROM deleted)",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn soft_delete_by_ap_object_id(&self, ap_object_id: &str) -> Result<u64, sqlx::Error> {
        // `ap_object_id` はUNIQUE制約があるため高々1行のみ対象。`deleted_at IS NULL`
        // ガードで重複Delete受信時の二重デクリメントを防ぐ（soft_delete_by_id参照）。
        let result = sqlx::query(
            "WITH deleted AS (
                 UPDATE posts SET deleted_at = NOW()
                 WHERE ap_object_id = $1 AND deleted_at IS NULL
                 RETURNING actor_id
             )
             UPDATE actors SET notes_count = GREATEST(notes_count - 1, 0)
             WHERE id = (SELECT actor_id FROM deleted)",
        )
        .bind(ap_object_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn soft_delete_by_at_uri(&self, at_uri: &str) -> Result<Option<(i64, i64)>, sqlx::Error> {
        // `decremented` CTEは最終SELECTからLEFT JOINで参照することで実行を保証する
        // （どこからも参照されないdata-modifying CTEはPostgresが実行しない可能性があるため）。
        sqlx::query_as::<_, (i64, i64)>(
            "WITH deleted AS (
                 UPDATE posts SET deleted_at = NOW() WHERE at_uri = $1 AND deleted_at IS NULL
                 RETURNING id, actor_id
             ),
             decremented AS (
                 UPDATE actors SET notes_count = GREATEST(notes_count - 1, 0)
                 WHERE id = (SELECT actor_id FROM deleted)
                 RETURNING 1
             )
             SELECT deleted.id, deleted.actor_id FROM deleted LEFT JOIN decremented ON true",
        )
        .bind(at_uri)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_by_seiran_uuid(
        &self,
        uuid: &str,
    ) -> Result<Option<(i64, Option<String>)>, sqlx::Error> {
        let row: Option<(i64, Option<String>)> = sqlx::query_as(
            "SELECT id, ap_object_id FROM posts WHERE seiran_post_uuid = $1 LIMIT 1",
        )
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn update_ap_object_id(&self, id: i64, ap_object_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE posts SET ap_object_id = $1 WHERE id = $2")
            .bind(ap_object_id)
            .bind(id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn find_id_by_at_uri(&self, at_uri: &str) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar::<_, i64>("SELECT id FROM posts WHERE at_uri = $1 LIMIT 1")
            .bind(at_uri)
            .fetch_optional(&self.pool)
            .await
    }

    async fn find_id_by_ap_or_at_uri(&self, uri: &str) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM posts WHERE ap_object_id = $1 OR at_uri = $1 LIMIT 1",
        )
        .bind(uri)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_id_and_actor_by_ap_object_id(
        &self,
        ap_object_id: &str,
    ) -> Result<Option<(i64, i64)>, sqlx::Error> {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT id, actor_id FROM posts WHERE ap_object_id = $1 AND deleted_at IS NULL LIMIT 1",
        )
        .bind(ap_object_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn find_id_and_actor_by_at_uri(
        &self,
        at_uri: &str,
    ) -> Result<Option<(i64, i64)>, sqlx::Error> {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT id, actor_id FROM posts WHERE at_uri = $1 AND deleted_at IS NULL LIMIT 1",
        )
        .bind(at_uri)
        .fetch_optional(&self.pool)
        .await
    }

    async fn insert_remote_with_dedup(
        &self,
        params: InsertRemoteWithDedupParams<'_>,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        // #237 相互一致マージ判定。seiranPostの申告（claimed_at_uri）を持つ投稿のみ、
        // ap_object_idをキーにした advisory lock でDB反映を直列化してから既存行を探す
        // （`docs/protocols.md` 5節）。申告の無い一般的なリモート投稿はこの分岐を通らず
        // 従来通り無条件・ノーロックでINSERTする（cross-column突合が不要なため）。
        if let Some(claimed_at_uri) = params.claimed_at_uri {
            sqlx::query("SELECT pg_advisory_xact_lock(2, hashtext($1))")
                .bind(params.ap_object_id)
                .execute(&mut *tx)
                .await?;

            let existing: Option<(i64, i64, Option<String>)> = sqlx::query_as(
                "SELECT id, actor_id, claimed_ap_object_id FROM posts WHERE at_uri = $1",
            )
            .bind(claimed_at_uri)
            .fetch_optional(&mut *tx)
            .await?;

            if let Some((existing_id, existing_actor_id, claimed_ap_object_id)) = existing {
                let mutual_match = claimed_ap_object_id.as_deref() == Some(params.ap_object_id);
                // 投稿者の一貫性チェック（簡略版）: 現時点ではオンメモリなアクター結婚
                // （#236アルゴリズムの投稿受信時適用）は未実装のため、両投稿の投稿者が
                // 既に同一actor行に解決されている場合のみマージする。不一致の場合は
                // マージせず孤立行のまま残す（#236側のアクター統合が別途成立すれば、
                // 将来の再突合で解消できる余地を残す設計、`docs/protocols.md` 5節）。
                if mutual_match && existing_actor_id == params.actor_id {
                    sqlx::query(
                        "UPDATE posts SET ap_object_id = $1, claimed_ap_object_id = NULL WHERE id = $2",
                    )
                    .bind(params.ap_object_id)
                    .bind(existing_id)
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    return Ok(());
                }
            }
        }

        let result = sqlx::query(
            "INSERT INTO posts (id, actor_id, body, content_html, ap_object_id, seiran_post_uuid, parent_original_post_id, reply_to_post_id, thread_root_post_id, created_at, emoji_map, visibility, quote_of_post_id, reply_to_ap_uri, reply_to_ref_status, quote_of_ap_uri, quote_of_ref_status, claimed_at_uri)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12::post_visibility_enum, $13, $14, $15::post_reference_status, $16, $17::post_reference_status, $18)
             ON CONFLICT (ap_object_id) DO NOTHING",
        )
        .bind(params.id)
        .bind(params.actor_id)
        .bind(params.body)
        .bind(params.content_html)
        .bind(params.ap_object_id)
        .bind(params.seiran_uuid)
        .bind(params.parent_original_post_id)
        .bind(params.reply_to_post_id)
        .bind(params.thread_root_post_id)
        .bind(params.created_at)
        .bind(params.emoji_map)
        .bind(params.visibility)
        .bind(params.quote_of_post_id)
        .bind(params.reply_to_ap_uri)
        .bind(params.reply_to_ref_status)
        .bind(params.quote_of_ap_uri)
        .bind(params.quote_of_ref_status)
        .bind(params.claimed_at_uri)
        .execute(&mut *tx)
        .await?;

        // ON CONFLICT DO NOTHINGで重複スキップされた場合はpost_recipientsもnotes_countも更新しない。
        if result.rows_affected() > 0 {
            if !params.recipient_actor_ids.is_empty() {
                sqlx::query(
                    "INSERT INTO post_recipients (post_id, actor_id) SELECT $1, unnest($2::bigint[])",
                )
                .bind(params.id)
                .bind(params.recipient_actor_ids)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query("UPDATE actors SET notes_count = notes_count + 1 WHERE id = $1")
                .bind(params.actor_id)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await
    }

    async fn finalize_post_merge(
        &self,
        survivor_id: i64,
        doomed_id: i64,
        ap_object_id: &str,
        at_uri: &str,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // UNIQUE(ap_object_id)/UNIQUE(at_uri)はNULL同士を衝突と見なさないため、
        // 先にdoomed側をNULL化してからsurvivor側へ確定値をセットする（順序が逆だと
        // 制約違反になる）。doomedは通常どちらか一方しか持たないが、両方NULL化しても
        // 安全（既にNULLな列への再代入は無害）。
        sqlx::query("UPDATE posts SET ap_object_id = NULL, at_uri = NULL WHERE id = $1")
            .bind(doomed_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE posts SET ap_object_id = $1, at_uri = $2, claimed_ap_object_id = NULL, claimed_at_uri = NULL WHERE id = $3",
        )
        .bind(ap_object_id)
        .bind(at_uri)
        .bind(survivor_id)
        .execute(&mut *tx)
        .await?;
        // deleted_atのNULL→非NULL遷移でtrg_posts_relation_counts_deleteが発火し、
        // 結婚前に2行がそれぞれ二重加算していた親側カウンタ（reply/quote/repost_count）を
        // 自動的に1つ分補正する（`docs/protocols.md` 5節参照）。
        sqlx::query(
            "UPDATE posts SET parent_original_post_id = $1, deleted_at = now() WHERE id = $2",
        )
        .bind(survivor_id)
        .bind(doomed_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await
    }

    async fn set_fedi_content_metadata(
        &self,
        post_id: i64,
        content_warning: Option<&str>,
        poll: Option<&serde_json::Value>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE posts SET content_warning = $2, poll = $3,
                 poll_fetched_at = CASE WHEN $3 IS NOT NULL THEN created_at ELSE poll_fetched_at END
             WHERE id = $1",
        )
        .bind(post_id)
        .bind(content_warning)
        .bind(poll)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn find_stale_remote_poll_post_ids(
        &self,
        candidates: &[(i64, DateTime<Utc>)],
    ) -> Result<Vec<i64>, sqlx::Error> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i64> = candidates.iter().map(|(id, _)| *id).collect();
        let thresholds: Vec<DateTime<Utc>> = candidates.iter().map(|(_, t)| *t).collect();
        sqlx::query_scalar(
            "SELECT p.id FROM posts p
             JOIN UNNEST($1::bigint[], $2::timestamptz[]) AS t(id, threshold) ON p.id = t.id
             WHERE p.poll IS NOT NULL AND p.poll_update_received = false
               AND p.poll_fetched_at < t.threshold",
        )
        .bind(&ids)
        .bind(&thresholds)
        .fetch_all(&self.pool)
        .await
    }
}

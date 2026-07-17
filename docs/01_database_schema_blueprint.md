# Doc 1. データベース・スキーマ計画書 (Database Schema Blueprint)

本ドキュメントは、`seiran` システムにおけるエンティティ間の関係性、データ型、および分散ネットワーク特有のインデックス戦略を定義する。
* **想定RDB:** PostgreSQL 15以降

---

## 1. テーブル定義 ＆ 物理データモデル

### 1.1 `users` (ローカル認証・アカウント管理)

```sql
CREATE TYPE user_role AS ENUM ('user', 'moderator', 'admin');

CREATE TABLE users (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    email         VARCHAR(255) UNIQUE NOT NULL,
    password_hash TEXT,
    role          user_role NOT NULL DEFAULT 'user',
    suspended_at  TIMESTAMPTZ,            -- 非 NULL = 凍結済み
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

ロール権限マトリクス:

| 操作 | user | moderator | admin |
|---|---|---|---|
| 投稿・閲覧 | ✓ | ✓ | ✓ |
| 通報の閲覧・処理 | ✗ | ✓ | ✓ |
| ユーザーの凍結（`suspended_at` 設定） | ✗ | ✓ | ✓ |
| 他者ポストの削除 | ✗ | ✓ | ✓ |
| サーバー設定変更（管理画面） | ✗ | ✗ | ✓ |
| ユーザーロール任命 | ✗ | ✗ | ✓ |
| カスタム絵文字管理 | ✗ | ✗ | ✓ |
| オブジェクトストレージ設定 | ✗ | ✗ | ✓ |

### 1.2 `actors` (統一アクターテーブル)
世界中のすべての「肉体」を管理する。ローカル、リモート、ブリッジ（影武者）のすべてがここに並列で格納される。

```sql
CREATE TYPE actor_type_enum AS ENUM (
    'local', 
    'remote_seiran', 
    'fedi', 
    'bsky', 
    'fedi_bridge_to_bsky', 
    'bsky_bridge_to_fedi'
);

CREATE TABLE actors (
    id BIGINT PRIMARY KEY, -- タイムスタンプ内包の統一アクターID（snowflake等）
    user_id BIGINT REFERENCES users(id) ON DELETE SET NULL, -- ローカルユーザーの場合のみ結合
    actor_type actor_type_enum NOT NULL,
    
    -- プロトコル固有の識別子（URI / DID）
    ap_uri VARCHAR(2048) UNIQUE, -- ActivityPubのActor URI (例: [https://example.com/users/foo](https://example.com/users/foo))
    at_did VARCHAR(255) UNIQUE,  -- AT ProtocolのDID (例: did:plc:abcdefg...)

    -- ATP リポジトリ状態（ローカルユーザーのみ。seiran が PDS としてリポジトリを保持するために必要）
    at_signing_key_pem TEXT,  -- リポジトリコミット署名用 P-256 秘密鍵（PEM）
    at_repo_cid TEXT,         -- 直近コミットの CID（commit オブジェクト全体、subscribeRepos の `prev` に使う）
    at_repo_rev TEXT,         -- 直近コミットの rev（TID、subscribeRepos の `since` に使う）
    -- 直近コミット時点の MST root CID（commit オブジェクトの `data` フィールド）。
    -- AT Protocol Sync 1.1 で必須の `#commit` イベント `prevData` フィールドを次回コミット時に
    -- 埋めるために保持する（§10.6 参照。無いと2回目以降の全コミットがリレーにリジェクトされる）。
    at_repo_data_cid TEXT,
    
    -- 表示用メタデータ
    username VARCHAR(255) NOT NULL, -- ユーザー名（@のあとの英数字）
    domain VARCHAR(255) NOT NULL,   -- インスタンスドメイン（例: mstdn.jp, bsky.social）
    display_name VARCHAR(255),
    avatar_url VARCHAR(2048),       -- リモートアクターはこちらを使用
    banner_url VARCHAR(2048),       -- リモートアクターはこちらを使用
    avatar_media_id BIGINT REFERENCES media_files(id) ON DELETE SET NULL, -- ローカルアクターのみ
    banner_media_id BIGINT REFERENCES media_files(id) ON DELETE SET NULL, -- ローカルアクターのみ
    bio TEXT,
    -- 表示名中のカスタム絵文字（`:shortcode:`）→画像URLマップ。AP Person の `tag` 配列
    -- （`type:"Emoji"`）から解決する。ローカルアクターは常に空オブジェクト。
    emoji_map JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- プロフィールのキーバリュー項目（#62、Mastodon 等の「プロフィールのメタデータ欄」に
    -- 相当）。`[{"name": "サイト", "value": "https://example.com"}, ...]`（順序を保持、
    -- 最大4件）。ローカルユーザーが編集した値、またはリモート Fedi アクターの AP Actor
    -- `attachment`（`type: "PropertyValue"`）から取り込んだ値。
    profile_fields JSONB NOT NULL DEFAULT '[]'::jsonb,
    
    -- 相互マッピング用外部キーポインタ
    seiran_pair_actor_id BIGINT REFERENCES actors(id) ON DELETE SET NULL, -- リモートseiranの対の行
    bridge_real_actor_id BIGINT REFERENCES actors(id) ON DELETE SET NULL, -- ブリッジ時の「本尊」の行
    
    created_at TIMESTAMP WITH TIME ZONE NOT NULL,
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL,

    -- 退会管理（#29 Phase A）
    withdrawn_at TIMESTAMP WITH TIME ZONE                                   -- NULL=現役 / NOT NULL=退会済み
);

-- インデックス戦略
CREATE INDEX idx_actors_type ON actors(actor_type);
CREATE INDEX idx_actors_pair ON actors(seiran_pair_actor_id) WHERE seiran_pair_actor_id IS NOT NULL;
CREATE INDEX idx_actors_bridge ON actors(bridge_real_actor_id) WHERE bridge_real_actor_id IS NOT NULL;
CREATE INDEX idx_actors_user_id ON actors(user_id);                         -- 認証エンドポイントの find_local_by_user_id 高速化
CREATE INDEX idx_actors_username_domain ON actors(username, domain);        -- WebFinger / プロフィール検索 / フォロー解決高速化
CREATE INDEX idx_actors_withdrawn_at ON actors(withdrawn_at) WHERE withdrawn_at IS NOT NULL;
```

**ローカルユーザー名の命名規則**（`seiran_common::username`、`register()`で強制）:
ローカルユーザー名は「ドメイン名の1ラベルとして成立する文字列」でなければならない
（英数字とハイフンのみ、先頭/末尾はハイフン不可、ピリオド不可、1〜63文字）。理由は2つ:
1. ATPハンドルは `{username}.{domain}` の形で組み立てるため、username自体がDNSラベルと
   して妥当でなければ不正なハンドルになる。
2. `@`で始まり途中に`@`を含まない文字列（ローカルID`@user`かATPハンドル
   `@user.bsky.social`か）を見たとき、`.`の有無でどちらかを判別できる。

加えて`list-relay`（リスト機能#63のプロキシアクター用）等の予約ユーザー名は
`register()`が明示的に拒否する（`RESERVED_LOCAL_USERNAMES`）。

### 1.3 `posts` (統一ポストテーブル)
宇宙の壁を超えて集約されるすべての投稿。未来補正タイムスタンプ付きのIDで完全に一次元ソートされる。編集競合を避けるため、更新系の動的データ（リアクション等）は別テーブルに完全分離する。

```sql
CREATE TYPE post_visibility_enum AS ENUM ('public', 'unlisted', 'followers_only', 'direct');

CREATE TABLE posts (
    id BIGINT PRIMARY KEY, -- 補正タイムスタンプ内包の統一ポストID（ソート主軸）
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    
    -- コンテンツ本体 (パターンB: プレーンテキスト1本保持)
    body TEXT NOT NULL,
    
    -- 3大リレーション（すべて最大1件のためフラットカラムで保持）
    reply_to_post_id BIGINT REFERENCES posts(id) ON DELETE SET NULL,  -- リプライ元（垂直軸）
    repost_of_post_id BIGINT REFERENCES posts(id) ON DELETE SET NULL, -- リポスト元（拡散）
    quote_of_post_id BIGINT REFERENCES posts(id) ON DELETE SET NULL,  -- 引用元（言及）
    
    -- 重複排除・関連リンク用の関係キー
    seiran_post_uuid VARCHAR(255) UNIQUE, -- 他seiranサーバー間での投稿マージ用共通UUID
    parent_original_post_id BIGINT REFERENCES posts(id) ON DELETE SET NULL, -- ループバック/ブリッジ重複時のオリジナルへのハードリンク
    
    -- プロトコル固有の識別子
    ap_object_id VARCHAR(2048) UNIQUE, -- ActivityPubのNote ID
    at_uri VARCHAR(2048) UNIQUE,       -- AT Protocol of the URI
    at_cid VARCHAR(255),               -- ATPのコンテンツハッシュ
    
    -- 静的・拡張メタデータ (プロトコル別変形レシピ、メディア情報などを格納)
    -- ※頻繁なUPDATEがかかる動的データはここに入れないこと
    metadata JSONB DEFAULT '{}'::jsonb NOT NULL, 
    -- 本文中のカスタム絵文字（`:shortcode:`）→画像URLマップ。AP Note の `tag` 配列
    -- （`type:"Emoji"`）から解決する。ローカル投稿は常に空オブジェクト。
    emoji_map JSONB NOT NULL DEFAULT '{}'::jsonb,
    
    -- 削除・変更管理
    deleted_at TIMESTAMP WITH TIME ZONE DEFAULT NULL, -- 論理削除フラグ
    atp_tombstone_cid VARCHAR(255) DEFAULT NULL,     -- ATPリポジトリ内での削除証明用CID
    
    created_at TIMESTAMP WITH TIME ZONE NOT NULL,
    inserted_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL,

    -- ローカルタイムライン高速化用の非正規化カラム（#DBパフォーマンス改善 2026-07-10）
    -- actors.actor_type = 'local' の複製。リモート流入が多い前提では
    -- 「posts を id 降順で全走査しつつ actors を JOIN して足切り」だと
    -- テーブルが育つほどローカル投稿を探すコストが際限なく増える。
    -- actor_type は投稿後に変わらないため posts 側に複製しても不整合しない。
    -- BEFORE INSERT トリガー trg_posts_set_is_local が自動設定するため、
    -- アプリケーションコードから明示的に書き込む必要はない。
    is_local BOOLEAN NOT NULL DEFAULT false,

    -- 配送先・可視性アイコン表示用（#配送先・可視性アイコン追加 2026-07-16）
    -- visibility: Fedi 受信ポストの Note.to/cc から判定した4値（seiran_common::ap::classify_ap_visibility）。
    -- ローカル投稿は現状常に 'public' 固定（可視性選択UIは将来課題）。
    visibility post_visibility_enum NOT NULL DEFAULT 'public',
    -- deliver_fedi/deliver_bsky: ローカル投稿作成時に実際に配送対象とした値の永続化。
    -- ap_object_id は deliver_to_fedi の値に関わらず常に生成される（投稿自身のAP識別子として
    -- 必要なため）ので「配送されたか」の判定には使えず、別途カラムで持つ。リモート受信ポストでは
    -- 意味を持たない（常にデフォルト値のまま）。
    deliver_fedi BOOLEAN NOT NULL DEFAULT true,
    deliver_bsky BOOLEAN NOT NULL DEFAULT true
);

-- リポスト重複制約: 同一ユーザーが同一ポストを取り消し前に再リポストできないようにする
-- 論理削除（deleted_at IS NULL）している行のみ対象。取り消し後は再リポスト可能。
CREATE UNIQUE INDEX idx_posts_unique_repost
    ON posts(actor_id, repost_of_post_id)
    WHERE repost_of_post_id IS NOT NULL AND deleted_at IS NULL;

-- posts.actor_id の actor_type を自動複製するトリガー（is_local 非正規化カラムの維持用）
CREATE OR REPLACE FUNCTION posts_set_is_local() RETURNS trigger AS $$
BEGIN
    SELECT (actor_type = 'local') INTO NEW.is_local
    FROM actors WHERE id = NEW.actor_id;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_posts_set_is_local
    BEFORE INSERT ON posts
    FOR EACH ROW
    EXECUTE FUNCTION posts_set_is_local();

-- インデックス戦略
CREATE INDEX idx_posts_actor_id ON posts(actor_id, id DESC) WHERE deleted_at IS NULL; -- recent_by_actor / context_* のインデックススキャン化
CREATE INDEX idx_posts_reply_to ON posts(reply_to_post_id) WHERE reply_to_post_id IS NOT NULL;
CREATE INDEX idx_posts_repost_of ON posts(repost_of_post_id) WHERE repost_of_post_id IS NOT NULL;
CREATE INDEX idx_posts_quote_of ON posts(quote_of_post_id) WHERE quote_of_post_id IS NOT NULL;
CREATE INDEX idx_posts_parent_original ON posts(parent_original_post_id) WHERE parent_original_post_id IS NOT NULL;
CREATE INDEX idx_posts_seiran_uuid ON posts(seiran_post_uuid) WHERE seiran_post_uuid IS NOT NULL;
CREATE INDEX idx_posts_local_active ON posts(id DESC) WHERE deleted_at IS NULL AND is_local = true; -- local_timeline 専用（#DBパフォーマンス改善 2026-07-10）
CREATE INDEX idx_posts_timeline_active ON posts(id) WHERE deleted_at IS NULL;

-- 全文検索用インデックス（pg_trgmを利用したTrigram部分一致インデックス）
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE INDEX idx_posts_body_trgm ON posts USING gin (body gin_trgm_ops);
```

#### 配送先・可視性アイコン表示（2026-07-16、閲覧制御は2026-07-17）

タイムライン上のポストカードに「配送先」「可視性」を示すアイコンを表示するための永続化。
アイコン対応: `public`=👥パブリック（アイコン無し表示）、`unlisted`=🤫ひかえめ、
`followers_only`=🔒️プライベート。

- **配送先（ローカル投稿のみ）**: 投稿作成リクエストの `deliver_to_fedi`/`deliver_to_bsky`
  （`CreateNoteRequest`）をそのまま `posts.deliver_fedi`/`deliver_bsky` に保存する
  （`crates/seiran-api/src/handlers/notes/mod.rs` の `create_regular_post` →
  `PostRepository::insert_full`）。`ap_object_id` は `deliver_to_fedi` の値に関わらずローカル
  投稿なら常に生成される（投稿自身のAP識別子として必要）ため、配送有無の判定には使えないことに
  注意。リポスト（`posts.repost_of_post_id` が非NULLの行）自体はこの2カラムを持たず常にデフォルト
  値のまま。`NoteCard` はリポストの中身を `renote` に埋め込まれた元ポストから描画するため、リポスト
  行自体の配送先は表示に使われない。
- **可視性（ローカル投稿・Fedi受信ポスト双方）**: ローカル投稿は投稿作成リクエストの `visibility`
  （`"public"`/`"unlisted"`/`"followers_only"`、`create_regular_post` で検証）をそのまま
  `posts.visibility` に保存する。`"direct"` はローカル投稿作成のスコープ外（Fedi受信専用）。
  Fedi受信ポストは AP `Create(Note)` 受信時（`handle_create_note`、
  `crates/seiran-common/src/jobs/inbound_activity_process.rs`）に Note の `to`/`cc` を
  `seiran_common::ap::classify_ap_visibility` で判定し保存する（判定ルール、Mastodon互換: `to` に
  `Public` があれば `public`、`cc` にのみあれば `unlisted`、どちらにも無く `to` にフォロワー
  コレクション（`.../followers` で終わるURI）があれば `followers_only`、それ以外は `direct`）。
  - **Bsky配送との相互排他（2026-07-17改訂）**: Bsky（AT Protocol）はプロトコル上
    followers_only（フォロワー限定）配信をサポートしないため、`visibility == "followers_only"`
    かつ Bsky配送が要求された場合、`create_regular_post` はエラーを返さず `deliver_bsky` を黙って
    `false` に読み替える（Misskey互換API保護。フロントを経由しない外部クライアントからの想定外
    リクエストにも対応するため）。`unlisted` は Bsky配送を許可する（当初は非public全体を不許可に
    していたが、リポストへの可視性継承との整合性のため緩和した）。`delivery.rs` の
    `deliver_regular_post`・`deliver_repost` でも同様のチェックを再度行う（二重防御、呼び出し元
    バグの検知用）。
  - **AP `to`/`cc` への反映**: `crates/seiran-common/src/ap/deliver.rs` の
    `build_create_note_activity` が `posts.visibility` に応じて to/cc を可変化する
    （`classify_ap_visibility` の逆写像）: `public`→to=[Public],cc=[followers] /
    `unlisted`→to=[followers],cc=[Public] / `followers_only`→to=[followers],cc=[]。
- **API公開**: `NoteResponse`（`to_note_response`）は `visibility` が `public` の場合は省略、
  それ以外は文字列で返す。`deliverFedi`/`deliverBsky` はローカル投稿（`actor_type == "local"`）
  の場合のみ含める。Misskey互換API（`to_misskey_note`）は `to_misskey_visibility` で
  `unlisted→"home"`, `followers_only→"followers"`, `direct→"specified"` にマッピングする。
- **フロント表示**: `NoteCard.tsx` が `lib/format.ts` の `deliveryBadges`/`visibilityBadge` を使い、
  既存のプロトコルバッジ（`protocolBadge`、🦋=Bsky/🌐=Fedi/🀄=seiran）と同じ `.protoBadge` スタイルで
  並べて表示する。配送先は 🌐=Fedi配送あり・🦋=Bsky配送あり、可視性は 🔒️=プライベート・
  🤫=ひかえめ（`public`/`direct` はアイコン無し）。
- **投稿作成UI**: `PostComposer.tsx` に可視性選択（👥パブリック/🤫ひかえめ/🔒️プライベート）を追加。
  Bsky配送ON中は非publicへの変更を、非public中はBsky配送ONを、それぞれ軽量な自作ポップオーバーで
  ブロックする（相互排他のUIガイド、実際の強制はサーバー側で行う）。

##### 可視性による閲覧制御（2026-07-17、`unlisted`のホーム表示は同日中に訂正）

`followers_only`/`direct` は投稿者本人または accepted フォロワーの閲覧者のみアクセスできる。
`unlisted` は**ローカル（グローバル）タイムラインからのみ除外**する。ホームタイムライン
（自分 + フォロー中のアクターの投稿）・プロフィール・パーマリンク・スレッド内では通常表示する
（Mastodon準拠）。当初は「ホームタイムラインからも除外」としていたが、フォロー関係にある相手の
ひかえめ投稿が home に出なくなる実害が確認されたため、`home_timeline` のクエリからは
`unlisted` 除外条件を外した（`local_timeline` のみ除外条件を維持）。

共通ガード式（`PostRepository` の各クエリの `WHERE` に追加、`follows` の複合PKでインデックス
オンリー解決可能なため新規インデックス不要）:
```sql
AND (
    p.visibility NOT IN ('followers_only', 'direct')
    OR p.actor_id = $viewer
    OR EXISTS (SELECT 1 FROM follows f
               WHERE f.follower_actor_id = $viewer AND f.target_actor_id = p.actor_id AND f.status = 'accepted')
)
```
`direct`（ローカル投稿UIからは選択不可だが既存のFedi受信データが残る）も暫定的に `followers_only`
と同じ厳格さでガードする（専用の宛先管理は将来課題）。`$viewer` が `NULL`（匿名）なら自己判定・
EXISTS ともに常に偽になるため、匿名は非公開投稿を一切閲覧できない。

- **`find_by_id` と `find_by_id_for_viewer` の使い分け**: `find_by_id` は可視性チェック無しの
  内部整合性チェック専用（`inbound_activity_process.rs`・`seiran-atp-repo/src/firehose.rs` の
  Undo(Like) 対象の author 引き当て等、閲覧者という概念が存在しない箇所）。HTTP公開エンドポイント
  （`get_note`/`get_note_ap`/`note_context`/`create_reaction`/`delete_reaction`/`pin_note`/
  Misskey互換の`notes_show`/`build_notifications`）は必ず `find_by_id_for_viewer` を使う。
- **`pinned_posts` の可視性ガード（2026-07-17）**: `PinnedPostsRepository::list_timeline_by_actor`
  （プロフィール画面のピン留め表示、`crates/seiran-api/src/handlers/users.rs`）に
  `viewer_actor_id` を追加し、`find_by_id_for_viewer` と同じガード式を適用した。AP向けの
  featured collection（`GET /users/:username/collections/featured`、
  `crates/seiran-federation-inbox/src/handlers/featured.rs`）は認証なしの完全匿名アクセスのため、
  `followers_only`/`direct` なピン留め投稿を無条件で除外する。Bskyプロフィールの `pinnedPost`
  同期（`resolve_bsky_pinned_post`、`crates/seiran-api/src/handlers/notes/mod.rs`）も、最新の
  ピン留め投稿が `followers_only`/`direct` の場合は同期をスキップする（Bskyはプロトコル上
  followers_only を表現できず、同期すると誰でも見える形で公開されてしまうため）。

##### リポスト・リプライの可視性継承（2026-07-17）

- **リポスト禁止**: `followers_only`/`direct` ポストはリポスト不可（`create_repost` が
  `403 PRIVATE_POST_NOT_REPOSTABLE` を返す。Mastodon/Misskey 同様、通常のエラー応答とし
  Bsky相互排他のような「黙った読み替え」対象にはしない）。
- **可視性継承**: `unlisted` ポストのリポストは `unlisted` を継承する（`public` はそのまま
  `public`）。リポストのvisibilityはクライアントが選べず、サーバーが元ポストから自動決定する
  （`PostRepository::insert_repost` の `visibility` 引数）。継承した可視性はリポスト行自身の
  `posts.visibility` に保存されるため、既存のタイムライン閲覧制御（`unlisted`除外・
  `followers_only`ガード）にリポスト行もそのまま乗る。AP `Announce` の `to`/`cc` もリポスト自身の
  可視性に応じて可変化する（`build_announce_activity`、`build_create_note_activity` と同じ
  `visibility_to_to_cc` ヘルパーを再利用）。
- **Bsky配送の二重防御**: `deliver_repost`（`crates/seiran-api/src/handlers/notes/delivery.rs`）は
  元ポストの `PostDeliveryMeta.visibility` を見て、`followers_only`/`direct` なら Bsky コミットを
  スキップする（`create_repost` の禁止ロジックにより通常は到達しないが、最終防御として実装）。
- **リプライの可視性継承**: 親が `followers_only` の場合、リプライは強制的に `followers_only`
  （黙った読み替え、Bsky配送も結果的に不可）。親が `unlisted` の場合、リプライは
  public/unlisted/followers_only いずれも選択可、デフォルトは親を継承し `unlisted`。親が
  `public`/`direct` の場合は制約なし（`ReplyContext::resolve_visibility`、
  `crates/seiran-api/src/handlers/notes/delivery.rs`）。
- **Fedi受信Announceの可視性判定（2026-07-17）**: `handle_announce`
  （`crates/seiran-common/src/jobs/inbound_activity_process.rs`）は Announce アクティビティ自身の
  `to`/`cc` を `classify_ap_visibility` で判定し、`insert_repost` の `visibility` にそのまま渡す
  （元ポストの可視性ではなく、そのリポストという行為自体が `to`/`cc` でどう公開されたかを見る）。
  以前は常に `public` 固定で保存していたが、これによりリモートから受信した非公開ブーストも
  ローカルの閲覧制御（`followers_only`/`direct`ガード、`unlisted`のローカルタイムライン除外）の
  対象になる。

### 1.4 `reactions` (リアクション独立テーブル)
コンフリクトやデッドタプルの増殖を防ぐため、ポストに対する動的な絵文字リアクション・Likeはすべて独立した行として `INSERT` / `DELETE` 管理する。

```sql
CREATE TABLE reactions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    
    -- リアクションの種別と内容（FediのLike/EmojiReact, ATPのLikeをすべて抽象化）
    reaction_type VARCHAR(50) NOT NULL DEFAULT 'emoji', -- 例: 'emoji', 'like'
    content VARCHAR(255) NOT NULL,                      -- 例: "👍", ":custom_emoji:"
    
    -- 外部宇宙での削除（Undo）追跡用ID
    ap_activity_id VARCHAR(2048) UNIQUE,
    at_uri VARCHAR(2048) UNIQUE,

    -- Fedi から受信したカスタム絵文字（content が ":shortcode:" 形式）の画像URL。
    -- AP の tag 配列（type:"Emoji"）から解決する。Unicode絵文字・ローカル送信分は NULL。
    emoji_url TEXT,

    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL,
    
    -- 1投稿につきユーザー1リアクションまで（Misskey 準拠。投稿元がローカル/リモートでも
    -- 挙動を統一するため全投稿一律で適用する。#22 初版は post_id+actor_id+content の
    -- ユニークだったため同一ユーザーが複数の異なる絵文字を付けられたが、連合先
    -- （Misskey 等）は1投稿1ユーザー1リアクションが前提のため後日この制約に変更した）
    UNIQUE(post_id, actor_id)
);

-- 右ペインの「リアクション集計・打ったメンツ一覧」を爆速化するための複合インデックス
CREATE INDEX idx_reactions_post_content ON reactions(post_id, content);
CREATE INDEX idx_reactions_actor_id ON reactions(actor_id);
```

#### 取り込み・公開（#22 実装メモ）
- **1投稿1ユーザー1リアクション**: `UNIQUE(post_id, actor_id)` により、同一ユーザーが同一投稿に付けられるリアクションは常に1個まで。別の絵文字を付けると `INSERT ... ON CONFLICT (post_id, actor_id) DO UPDATE` により既存のリアクションが新しい絵文字で**上書き（切り替え）**される（Misskey がリアクション変更時に `Undo` → 新規 `EmojiReact` を送る挙動と整合）。
- **AP 受信**: federation-inbox の `handle_reaction` が `Like`/`EmojiReact` の両 wire type を受信する。**Misskey は絵文字リアクション（Unicode・カスタム絵文字とも）でも AP の `type` を "Like" 固定で送り `EmojiReact` 型は使わない**ため、種別判定は wire type ではなく内容フィールドで行う：`content`（無ければ `_misskey_reaction`）を読み、どちらも無い場合のみ Mastodon 等の素のお気に入りとみなし `content = "❤️"` とする。`content` が `❤️` なら `reaction_type = 'like'`、それ以外は `'emoji'`。`content` が `:shortcode:` 形式（カスタム絵文字）の場合、activity の `tag` 配列（`[{type:"Emoji", name:":shortcode:", icon:{url:"..."}}]`）から一致する要素を探し `icon.url` を `emoji_url` に保存する（`extract_emoji_tag_url`、見つからなければ `NULL`）。`object` の URI から対象ローカルポスト（`posts.ap_object_id`）を解決して INSERT（＝上記の上書き）する。`ap_activity_id` に元アクティビティ ID を保存し、`Undo(Like)` / `Undo(EmojiReact)` で DELETE する。未知ポストへのリアクションは無視する。
- **API 公開**: ノート系 API（`NoteResponse.reactions`）が `[{ emoji, count, reactedByMe, emojiUrl? }]`（`content` ごとの件数、多い順。`reactedByMe` は認証ユーザー自身がその絵文字を付け済みかどうか。配列中で `reactedByMe: true` になり得るのは最大1件。`emojiUrl` は Fedi 受信のカスタム絵文字のみ付与し、同一 `content` の行が複数ドメインの同名カスタム絵文字を含む場合は代表の1つのURLになる簡略仕様）を返す。空なら省略。
- **ローカルユーザーによる追加/取消**: `POST /api/notes/:id/reactions`（body `{ content }`）でリアクションを追加（既存の自分のリアクションがあれば切り替え）、`DELETE /api/notes/:id/reactions/:content` で取消する（`reaction_type = 'emoji'` 固定、`ap_activity_id` はローカルで発行した `https://{domain}/activities/reactions/{post_id}-{actor_id}-{epoch_millis}`、`emoji_url` は常に `NULL`）。`content` は `emojis` crate（Unicode公式データ準拠）による完全一致で Unicode 絵文字のみ許可し、`:shortcode:` やプレーンテキストは拒否する（カスタム絵文字ピッカー未実装のため。Doc3 §9.6 参照）。**AP `Like`/`EmojiReact` の outbound 配信を実装済み**（`deliver_ap_reaction`/`deliver_ap_undo_reaction`、対象ポスト著者 + 自分の Fedi フォロワー全員へ配送、詳細は Doc3 §9.6）。**ATP（Bluesky）へも outbound 配信する**（下記）。追加時、ポスト著者がリアクションした本人以外であれば StreamHub 経由で `reaction` イベント（`{ postId, emoji, actor }`）を通知する。
- **リアクションのリアルタイム配信**: 上記の通知（`"reaction"`）とは別に、ローカル追加/取消・AP 受信 `EmojiReact`/`Like`・`Undo`・ATP `app.bsky.feed.like` の受信/削除のいずれでも `seiran_common::streaming::broadcast_reaction_update` を呼び、`{ "type": "noteUpdated", "body": { postId, reactions: [{emoji,count,emojiUrl}], reactorActorId, reactorEmoji } }` を著者 + accepted なローカルフォロワーへ送出する。フロントはこれを受けて `NoteCard` のリアクション表示をその場で更新する（詳細は Doc2 §2.7・§2.8）。カスタム絵文字（`emojiUrl` 付き）は `ReactionChips` が `img` 表示し、ローカルユーザーが自分でまだ付けていないカスタム絵文字チップはクリック不可にする（送信すると必ずバリデーションで弾かれるため）。
- **ATP（Bluesky）連携（outbound）**: `AtpCommitService::commit_like`（`crates/seiran-common/src/atp/service.rs`）が対象ポストの `at_uri`/`at_cid` を subject として `app.bsky.feed.like` レコードをコミットする。ATP には絵文字リアクションの概念が無い（Like のみ）ため、どの絵文字でも Like として送る。実際の絵文字は非標準の拡張フィールド `emoji` としてベストエフォートで載せる（Bluesky 公式クライアントは無視するだけのはず。実際に生成した署名付きレコードで UTF-8 エンコードを確認済み）。生成した at_uri（`at://did/app.bsky.feed.like/rkey`）を `reactions.at_uri` に保存し、`ap_activity_id` の ATP 版として扱う。切替（別の絵文字への変更）は `delete_atp_like` で旧 Like を削除してから `commit_like` で新規作成（ATP 側にも「1投稿1いいね」を反映）。対象ポストが ATP 上に存在しない（`at_uri`/`at_cid` が無い）場合は配信しない。いずれも API レスポンスをブロックしない fire-and-forget（`tokio::spawn`）。
- **ATP（Bluesky）連携（inbound）**: `crates/seiran-atp-repo/src/firehose.rs` は Bluesky 公式の **Jetstream**（`wss://jetstream1.us-east.bsky.network/subscribe`、Relay Firehose を購読して dag-cbor → JSON にデコード済みのイベントを配信する軽量サービス）に `wantedCollections=app.bsky.feed.post&wantedCollections=app.bsky.feed.like` を指定して接続する。この2 collection 以外（repost/follow/block/profile/カスタムlexicon等）はサーバー側で除外されるため、受信自体しない（旧: Relay の生 Firehose に直結して全collectionを受信し、CBOR/CARを自前デコードしていた。Jetstream 移行によりその自前デコード処理は不要になった）。`app.bsky.feed.like` の create イベントは、Jetstream が既にJSON化して同梱してくる `record.subject.uri` を見て `posts.at_uri` と一致するものだけを取り込む（**Like の対象は任意の投稿なので、投稿の場合と異なり liker の DID がローカルで既知かどうかでは絞り込めない**。一致しなければ何もしない）。実トラフィックで確認したところ Like イベントはグローバルで概ね600〜700件/秒あり、`wantedCollections` で他collectionは除外できてもLike自体の全件判定は毎回発生する（DIDベースの絞り込み `wantedDids` は未導入。今後の課題）。一致した場合、liker のアクターが未知なら AppView からプロフィールを取得して `upsert_remote_bsky` で作成し、`reactions` へ `content` は非標準 `emoji` フィールドがあればそれ、無ければ絵文字ピッカーと同じ `"❤️"`（VS16付きハート）で INSERT（`at_uri` に受信した Like レコードの at_uri を保存）。delete イベントは `at://did/app.bsky.feed.like/rkey` を組み立てて `reactions.at_uri` で直接 DELETE する（AP の `Undo` と同型、レコード本体は不要）。
- **ATP（Bluesky）連携（投稿取り込み）**: 同じ Jetstream 接続で `app.bsky.feed.post` の create も検知する。DID が既知のローカル `actors`（`at_did` 一致）であれば、Jetstream イベントに同梱される `record.text`/`record.createdAt` をそのまま `posts` へ保存する。Jetstream はほぼリアルタイムでレコード本体を配信するため、旧実装にあった「AppViewへの再取得＋インデックス遅延リトライ（2s/5s/10s）」は不要になった。`record.reply.parent.uri` が付いている場合はリプライとみなし、その URI が `posts.at_uri` として既知（＝こちらの投稿、または既に取り込み済みの Bsky 投稿への返信）であれば `posts.reply_to_post_id` を設定する。親が未知の場合（無関係な Bsky ユーザー同士のリプライ等）は通常投稿として保存する。
- **本文・表示名中のカスタム絵文字（`emoji_map`）**: `posts.emoji_map`/`actors.emoji_map` は、AP 受信時（`handle_create_note`/`upsert_remote_fedi_actor`）に Note/Person の `tag` 配列（`type:"Emoji"`）から `seiran_common::ap::build_emoji_map` で `{shortcode: 画像URL}` を抽出して保存する（ローカル投稿・ローカルアクターは常に空オブジェクト）。API 公開時（`to_note_response`）は投稿の `emoji_map` と投稿者の `emoji_map` を統合して `NoteResponse.emojis` として返す。フロントは `EmojiText` コンポーネント（`frontend/src/components/note/EmojiText.tsx`）で本文・表示名中の `:shortcode:` をこのマップと突き合わせ、解決できるものだけ `img` に置換する（解決できない場合はテキストのまま）。詳細は Doc3 §9.7。既知の制約: `create_note`/`create_repost` API のレスポンス直後の `NoteResponse`（常にローカルユーザー自身の投稿）は `emojis` を空のまま返す（ローカル表示名にカスタム絵文字は無いため）。プロフィールページ単体の表示名・`ReplyIndicator` の「返信先」表示名は今回未対応（今後の課題）。

### 1.5 `follows` (フォロー関係テーブル)
プロトコルごとの「配送」の宛先リストとしても機能する。

```sql
CREATE TABLE follows (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    follower_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    target_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    status VARCHAR(10) NOT NULL DEFAULT 'accepted',  -- 'accepted' | 'pending'
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL,
    
    UNIQUE(follower_actor_id, target_actor_id)
);

CREATE INDEX idx_follows_follower ON follows(follower_actor_id);
CREATE INDEX idx_follows_target ON follows(target_actor_id);
CREATE INDEX idx_follows_target_follower ON follows(target_actor_id, follower_actor_id) WHERE status = 'accepted'; -- 自分のフォロワー一覧 / AP配送フォロワー取得高速化
CREATE INDEX idx_follows_follower_accepted ON follows(follower_actor_id) INCLUDE (target_actor_id) WHERE status = 'accepted'; -- ホームTL「自分のフォロー先」取得を index-only scan 化（#DBパフォーマンス改善 2026-07-10）
```

### 1.6 `email_verifications` (メールアドレス確認フロー)

ユーザー登録の 2 ステップフローで使用。確認メール送信後、24 時間以内に登録を完了しなければならない。

```sql
CREATE TABLE email_verifications (
    id          BIGINT PRIMARY KEY,
    email       TEXT NOT NULL,
    token       UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    expires_at  TIMESTAMPTZ NOT NULL DEFAULT now() + INTERVAL '24 hours',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

- `token`: 確認メールの URL に埋め込まれる UUID。`GET /api/auth/verify-token?token=...` で照合。
- トークンの有効性は「レコードが存在し `expires_at` が未来」だけで判断する。
- 登録完了時（`POST /api/auth/register`）にレコードを DELETE してトークンを消費する。

---

### 1.7 `password_resets` (パスワードリセット)

パスワードリセットフローで使用。リセットリンクの有効期限は **1 時間**。

```sql
CREATE TABLE password_resets (
    id         BIGINT PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token      UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + INTERVAL '1 hour',
    used_at    TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_password_resets_token ON password_resets(token);
```

- `token`: リセットメールの URL に埋め込まれる UUID（DB が自動生成）。
- `used_at`: リセット完了時に `NOW()` を記録（NULL = 未使用）。使い捨てトークン。
- トークンの有効性は「`used_at IS NULL` かつ `expires_at > NOW()`」で判断。

---

### 1.8 `storage_providers` (オブジェクトストレージプロバイダー)

管理画面から登録する S3 互換オブジェクトストレージの設定。複数登録可能で、`id` 順に使用し容量上限に近づいたら次のプロバイダーへフォールバックする。

```sql
CREATE TABLE storage_providers (
    id           BIGINT PRIMARY KEY,
    name         VARCHAR(255) NOT NULL,      -- 管理者が付けるラベル
    endpoint     VARCHAR(1024) NOT NULL,     -- S3 endpoint URL
    bucket       VARCHAR(255) NOT NULL,
    region       VARCHAR(100) NOT NULL DEFAULT 'auto',
    access_key   VARCHAR(255) NOT NULL,
    secret_key   TEXT NOT NULL,              -- AES-256-GCM で暗号化して格納（復号鍵は secrets.toml の encryption_key）
    public_url   VARCHAR(1024) NOT NULL,     -- 公開アクセス用ベース URL
    capacity_mb  BIGINT,                     -- NULL = 無制限。設定時は使用量合計がこれを超えたら次のプロバイダーへ
    is_active    BOOLEAN NOT NULL DEFAULT true,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

**ストレージ選択アルゴリズム:**
1. `is_active = true` のプロバイダーを `id` 昇順で走査
2. `capacity_mb` が NULL → 無条件で選択
3. `capacity_mb` が設定済み → `SELECT SUM(size) FROM media_files WHERE storage_provider_id = this.id` を計算し、合計 + 今回ファイルサイズ ≤ `capacity_mb * 1024 * 1024` なら選択。超過なら次へ
4. 全プロバイダー満杯 → アップロードエラー

---

### 1.8 `media_files` (メディアファイル・グローバル重複排除)

アップロードされた画像・動画・音声のメタデータ。オーナー概念を持たない。

```sql
CREATE TABLE media_files (
    id                   BIGINT PRIMARY KEY,
    storage_provider_id  BIGINT NOT NULL REFERENCES storage_providers(id),
    sha256               CHAR(64) NOT NULL,
    blurhash             VARCHAR(100),                -- 画像・動画サムネイルのみ。音声は NULL
    size                 BIGINT NOT NULL,             -- バイト数
    width                INT,                          -- 画像・動画のみ。音声は NULL
    height               INT,                          -- 画像・動画のみ。音声は NULL
    mime_type            VARCHAR(50) NOT NULL DEFAULT 'image/webp',
    storage_key          VARCHAR(1024) NOT NULL,
    duration_ms          INT,                          -- 動画・音声の再生時間。画像は NULL
    thumbnail_key        TEXT,                         -- 動画サムネイル(WebP)の storage_key。画像・音声は NULL
    bsky_video_job_id    TEXT,                         -- Bsky公式動画パイプラインのuploadVideo jobId
    bsky_video_cid       TEXT,                         -- getJobStatus完了時に得られるblob CID
    bsky_video_status    TEXT,                         -- NULL(対象外)/'pending'/'ready'/'failed'
    uploaded_by_actor_id BIGINT REFERENCES actors(id) ON DELETE SET NULL, -- GC・監査用（オーナーシップではない）
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (sha256, blurhash),
    UNIQUE (storage_provider_id, storage_key)
);

CREATE INDEX idx_media_files_sha256 ON media_files(sha256);
```

- 画像: `width`/`height`/`blurhash` は必須で埋まる（従来通り）。
- 動画: `width`/`height` は ffprobe で取得した実解像度、`blurhash` は抽出した
  サムネイルフレームを画像処理パイプラインに通して計算した値、`thumbnail_key` に
  サムネイル WebP の storage_key、`duration_ms` に再生時間を保存する。
  本体は無変換（トランスコードなし）でそのまま保存する。
- 音声: `width`/`height`/`blurhash`/`thumbnail_key` は全て NULL、`duration_ms` のみ
  埋まる。本体は無変換でそのまま保存する。
- 動画かつBsky配信ありでアップロードされた場合、`bsky_video_job_id`/
  `bsky_video_cid`/`bsky_video_status`にBluesky公式動画パイプライン
  （`app.bsky.video.uploadVideo`）との連携状態を保持する。`bsky_video_status`が
  `'ready'`になって初めて、そのポストのATP配信で`app.bsky.embed.video`が使われる
  （それ以外は`app.bsky.embed.external`にフォールバック）。詳細は
  `docs/03_multi_protocol_engine_specification.md`参照。

**重複排除フロー:**
- 画像: 変換後バイト列の SHA-256 と blurhash を計算し `(sha256, blurhash)` の
  複合一致で重複排除（従来通り）。
- 動画: 本体（無変換）の SHA-256 と、サムネイルフレームから計算した blurhash の
  組み合わせで重複排除（決定的に同じフレームが抽出されるため再アップロードでも一致する）。
- 音声: `blurhash` が概念上存在しない（常に NULL）ため、SHA-256 のみで重複排除する
  （`blurhash IS NULL` 条件付き検索）。

**画像処理仕様（変換後の保存寸法）:**

| 用途 | 変換処理 | 保存寸法 |
|---|---|---|
| アバター | 中央正方形クロップ → WebP | 600 × 600 px |
| バナー | fit: inside → WebP | 横最大 2048 × 縦最大 768 px |
| カスタム絵文字 | fit: inside → WebP | 横最大 384 × 縦最大 64 px |
| ポスト添付（画像） | fit: inside → WebP | 長辺最大 2048 px |
| ポスト添付（動画サムネイル） | ffmpeg でフレーム抽出 → 画像処理と同じ fit: inside → WebP | 長辺最大 2048 px |

- 画像はサムネイルを生成しない。オリジナルは保持しない。WebP 1種のみ保存する。
- アニメーション画像はアニメ WebP として保持する。
- 動画・音声は ffmpeg でメタデータ（解像度・再生時間）抽出とサムネイル抽出のみ行い、
  本体のトランスコードは行わない（原本バイナリをそのまま保存）。
- アップロードサイズ上限は 100MB（`docker/nginx*.conf` の `client_max_body_size` と
  axum 側 `DefaultBodyLimit` を参照）。

**GC（ガベージコレクション）:**

定期ジョブ（例: 1日1回）が以下の孤立ファイルを検出し、object storage DELETE → `media_files` DELETE の順で回収する。

```sql
SELECT id, storage_provider_id, storage_key
FROM media_files
WHERE created_at < now() - INTERVAL '7 days'
  AND id NOT IN (
    SELECT media_file_id FROM post_attachments
    UNION
    SELECT avatar_media_id FROM actors WHERE avatar_media_id IS NOT NULL
    UNION
    SELECT banner_media_id FROM actors WHERE banner_media_id IS NOT NULL
    UNION
    SELECT media_file_id FROM custom_emojis
  );
```

**クォータ（利用容量）:**

「利用者」テーブルは持たない。クォータは実参照テーブル（`post_attachments`・`actors.avatar_media_id/banner_media_id`）から distinct な `media_file_id` を集計して算出する。チェックはポスト送信・プロフィール保存のタイミングで行う（アップロード時ではなく使う瞬間に判定）。

---

### 1.9 `post_attachments` (ポスト添付メディア)

```sql
CREATE TABLE post_attachments (
    post_id                BIGINT   NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    media_file_id          BIGINT   REFERENCES media_files(id),  -- ローカル投稿: あり／リモート受信: NULL
    position               SMALLINT NOT NULL,   -- 表示順（0 始まり）
    alt_text               TEXT,
    remote_url             TEXT,    -- リモート受信投稿の添付 URL（S3 に保存せず参照のみ）
    remote_mime_type       TEXT,    -- リモート受信投稿の実 MIME タイプ（AP attachment の mediaType 由来）
    remote_thumbnail_url   TEXT,    -- リモート受信投稿のサムネイル URL（本体と別URLの場合のみ。例: Bsky動画のHLSサムネイル）
    PRIMARY KEY (post_id, position)
);

CREATE INDEX idx_post_attachments_media ON post_attachments(media_file_id);
```

- ローカル投稿: `media_file_id` あり、`remote_*` 列は全て NULL（種別は `media_files.mime_type` を参照）
- AP(Fedi)受信投稿: `media_file_id` は NULL、`remote_url` に添付 URL、`remote_mime_type` に AP attachment の `mediaType`（欠落時は URL 拡張子から推測した値、判別不能なら NULL）を保存する。画像・動画・音声いずれも同じ仕組みで保持し、API レスポンスの `mime_type` から notecard 側で `<img>`/`<video>`/`<audio>` を出し分ける。
- ATP(Bsky)受信投稿: `media_file_id` は NULL。Jetstreamで受信した `record.embed` を解析し、`app.bsky.embed.images` は Bluesky CDN の画像URL（`https://cdn.bsky.app/img/feed_fullsize/plain/{did}/{cid}`）を `remote_url` に、`app.bsky.embed.video` はBluesky公式の動画パイプラインが生成するHLSプレイリストURL（`https://video.bsky.app/watch/{did}/{cid}/playlist.m3u8`、`remote_mime_type` は `application/vnd.apple.mpegurl`）を `remote_url` に、対応するサムネイルJPEG URL（`.../thumbnail.jpg`）を `remote_thumbnail_url` に保存する。いずれもDID+blob CIDのみから決定的に組み立てられるURLで、Bluesky AppViewへの追加問い合わせは不要（`crates/seiran-atp-repo/src/firehose.rs` の `parse_bsky_embed_attachments`）。`app.bsky.embed.recordWithMedia`（引用+メディア）は `media` フィールドを再帰的に解決する。フロントはHLSプレイリストを `hls.js` 経由で再生する（Safari はネイティブHLS再生）。
- 1ポストあたりの最大添付数は **4枚**（AT Protocol `app.bsky.embed.images` の上限に準拠）

---

### 1.10 `custom_emojis` (カスタム絵文字)

管理者のみが登録・削除できる。

```sql
CREATE TABLE custom_emojis (
    id            BIGINT PRIMARY KEY,
    shortcode     VARCHAR(100) NOT NULL UNIQUE,  -- コロン不要: "blobcat" など
    media_file_id BIGINT NOT NULL REFERENCES media_files(id),
    category      VARCHAR(100),
    tags          TEXT[] NOT NULL DEFAULT '{}',   -- #49: 複数タグ可・ホワイトスペース以外の文字を許可
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- タグ絞り込み用（配列包含）
CREATE INDEX idx_custom_emojis_tags ON custom_emojis USING GIN (tags);
```

- **タグ（#49）**: 1絵文字に複数タグ、別々の絵文字に同じタグを付けられる。使用文字はホワイトスペース以外すべて許可（`-` や日本語も可）。絵文字ピッカーの部分一致マッチ対象。管理 API `POST /api/admin/emojis`（作成時）と `PATCH /api/admin/emojis/:id`（後から編集）で設定でき、`GET /api/admin/emojis` が返す。サーバー側でトリム・空要素除去・重複除去を行う。

### 1.11 `site_settings` (サイト設定・キーバリューストア)

管理者 API（`GET/PATCH /api/admin/site-settings`）で変更可能なサーバー設定を格納する汎用 KV テーブル。

```sql
CREATE TABLE site_settings (
    key        VARCHAR(64) PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL
);
```

主要キー一覧:

| キー | 型（文字列表現） | デフォルト | 説明 |
|---|---|---|---|
| `smtp_host` | string | なし | SMTP サーバーホスト名 |
| `smtp_port` | string（数値） | `"587"` | SMTP ポート番号 |
| `smtp_username` | string | なし | SMTP 認証ユーザー名 |
| `smtp_password` | string | なし | SMTP 認証パスワード（API レスポンスでは返さない） |
| `smtp_from` | string | なし | 送信元メールアドレス |
| `smtp_tls` | `"tls"` / `"starttls"` | `"starttls"` | TLS モード |
| `require_email_verification` | `"true"` / `"false"` | `"false"` | メール確認必須フラグ |
| `jetstream_cursor` | string（数値） | なし | 直近処理した Bluesky Jetstream イベントの `time_us`（マイクロ秒 Unix タイムスタンプ）。管理者 API 経由では変更不可、`seiran-atp-repo`（`firehose.rs`）が5秒間隔で自動更新・再接続時に読み出す（Doc3 §14 参照） |
| `jetstream_wanted_dids_touch` | `"1"` 固定 | なし | Jetstream の `wantedDids` 絞り込みリスト再構築トリガー。値自体は使わず `updated_at` だけを「変更バージョン」として利用する。ATPフォロー増減・リストメンバー増減・リスト削除・ローカルユーザー退会のたびに更新され、`firehose`側が30秒間隔でポーリングして変化を検知する（Doc3 §14.2 参照） |

`smtp_host` が未設定の場合、`POST /api/auth/verify-email` は HTTP 503 + `{"code": "SMTP_NOT_CONFIGURED"}` を返す。

`jetstream_cursor` は管理者 API（`GET/PATCH /api/admin/site-settings`）の対象外で、他のキーとは異なりユーザー設定ではなく内部状態の永続化用途。

### 1.12 `notifications` (通知永続化テーブル)

従来「クイック通知」は WebSocket ライブ配信のみで永続化しておらず、フロントエンドの
インメモリ配列（最大100件、リロードで消滅）に頼っていた。マイケルの要望
（2026-07-15: 「一度見ると消えてしまうのはもったいない」）を受け、通知を DB に永続化し、
新着順の無限スクロールで過去分も遡れるようにした。エンドポイントは Misskey API 互換の
`POST /api/i/notifications`（Doc3 §5.8）。

```sql
CREATE TABLE notifications (
    id                 BIGINT PRIMARY KEY,  -- Snowflake ID（自然に時系列ソートされる）
    recipient_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    type               VARCHAR(32) NOT NULL,  -- 'follow' | 'reaction' | 'followRequestAccepted'
    notifier_actor_id  BIGINT REFERENCES actors(id) ON DELETE SET NULL,
    note_id            BIGINT REFERENCES posts(id) ON DELETE SET NULL,
    reaction           VARCHAR(255),  -- type='reaction' の場合のみ（リアクション内容）
    reaction_emoji_url TEXT,          -- type='reaction' かつカスタム絵文字の場合のみ（下記参照）
    is_read            BOOLEAN NOT NULL DEFAULT false,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    source_uri         VARCHAR(2048)  -- 発生源イベントの一意識別子（下記参照、無ければNULL）
);

CREATE INDEX idx_notifications_recipient ON notifications (recipient_actor_id, id DESC);
CREATE INDEX idx_notifications_recipient_unread ON notifications (recipient_actor_id) WHERE NOT is_read;
CREATE UNIQUE INDEX idx_notifications_source_uri ON notifications (source_uri) WHERE source_uri IS NOT NULL;
```

- **書き込み元**: リアクション作成（`handlers::notes::create_reaction`、ローカル/Jetstream
  Like inbound とも）、AP `Follow` 受信（`jobs::inbound_activity_process::handle_follow`）、
  AP `Accept(Follow)` 受信（`handle_accept`、type は `followRequestAccepted`）の各経路で
  `NotificationRepository::insert` を呼ぶ。書き込み失敗はログのみで本処理をブロックしない
  （通知はベストエフォート）。
- **`source_uri`（2026-07-16 追加）**: `firehose`/`federation-worker`の複数起動（Doc6
  既知の課題）で同一イベントが複線受信された場合に、通知が重複してDBに残る不具合への
  対処。ATP Likeイベントは`at_uri`（`app.bsky.feed.like`のrkey由来、1いいね=1レコードで
  一意）、AP Reactionイベントは`ap_activity_id`を`source_uri`として保存し、部分ユニーク
  インデックスで重複INSERTを`ON CONFLICT ... DO NOTHING`により無視する。follow系・
  ローカルリアクションの通知は`source_uri`を持たない（`NULL`同士は一意制約上区別され、
  何行あっても衝突しない）。
- **読み取り**: `NotificationRepository::list` が `until_id`/`since_id` カーソルで
  `id DESC` 取得する（他のタイムライン系クエリと同じカーソルページネーション規約）。
  `mark_all_read` は `POST /api/i/notifications` の `markAsRead`（デフォルト true）で
  呼ばれ、パネルを開いた時点の全件を既読にする。
- **カスタム絵文字リアクションの画像表示（`reaction_emoji_url` の非正規化保存）**: 初回実装は
  `notifications.reaction`（生文字列）しか保存せず、表示時に対象投稿の**現在の**リアクション
  集計（`reactions` テーブル、`fetch_reactions_map`）から画像URLを都度引いていた。しかし
  `reactions` は `UNIQUE(post_id, actor_id)` で1人1投稿1リアクションのため、同じアクターが
  後で別の絵文字へ切り替えると過去の行は上書きされて消え、古い通知の画像が解決できなくなる
  不具合があった（マイケル指摘・2026-07-15、同一アクターが短時間に何度もリアクションを
  切り替えるケースで顕在化）。マイグレーション `20260716020000_notifications_reaction_emoji_url.sql`
  で `reaction_emoji_url` を追加し、通知 INSERT 時点で確定している画像URLをこのテーブル自身
  にも非正規化保存するよう変更。読み取り時（`convert::build_notifications`）は、ノート単位で
  共有キャッシュした `MisskeyNote.reactionEmojis` にこの通知固有の `(reaction,
  reaction_emoji_url)` を上書き挿入することで、他の通知や投稿の現在状態に関わらず常に
  正しい画像URLが解決される。**既知の制約**: このマイグレーション適用前に作成された既存の
  通知データは `reaction_emoji_url` が `NULL` のままのため遡って修正されない（当時の
  URLを再構築する手段が無いため）。当該投稿の現在のリアクション内容とたまたま一致する場合
  のみ従来通りの都度解決にフォールバックする。
- ローカル同士のフォローは即時 `accepted` になり AP `Follow`/`Accept` を経由しないため、
  現状 `follow`/`followRequestAccepted` 通知はリモートからのフォローのみで発生する。

### 1.13 `pinned_posts` (ピン留めポストテーブル、#61)

ローカルユーザーが自分のポストを最大5件までピン留めできる機能（マイケル仕様: 5件を
超えると最古のピン留めから自動的に外れる）。加えて、Fedi/Bsky の**リモートアクター**の
プロフィールをローカルで閲覧した際に、そのアクター自身が設定しているピン留め投稿を
同期・表示するためのキャッシュストアとしても同じテーブルを使う。

```sql
CREATE TABLE pinned_posts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    pinned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (actor_id, post_id)
);

CREATE INDEX idx_pinned_posts_actor ON pinned_posts (actor_id, pinned_at DESC);
```

- **ローカルユーザーの pin/unpin**: `POST`/`DELETE /api/notes/:id/pin`
  （`handlers::notes::pin_note`/`unpin_note`）が `PinnedPostsRepository::pin`/`unpin` を呼ぶ。
  `pin` は INSERT 後に `actor_id` ごと `pinned_at DESC OFFSET 5` で6件目以降を `DELETE ...
  RETURNING post_id` し、追い出された `post_id` を返す（5件超過時の自動追い出し、DBトリガー
  ではなくアプリ層で実施）。自分の投稿以外は403（`post.actor_id != me.actor_id`）。
- **リモートアクターの同期**: `PinnedPostsRepository::sync_from_remote(actor_id, post_ids,
  now)` が、渡された `post_ids`（同期元での並び順、先頭ほど優先）で該当 `actor_id` の行を
  丸ごと洗い替える（`post_ids` に無い既存行を DELETE → 無い分を INSERT、`pinned_at` は
  並び順を保つよう `now` から順にずらして採番）。呼び出し元はプロフィール表示ハンドラ
  （`build_profile_response`/`fetch_bsky_profile_from_appview`、詳細は Doc3 §10）。
- **API 公開**: `ProfileResponse.pinned_posts`（`recent_posts` と同じ `NoteResponse` 形式）
  として返す。自分自身のプロフィールを見ている場合、`recent_posts`/`pinned_posts` 双方の
  各要素に `pinned_by_me` を付与する（ピン留めボタンの状態表示用、それ以外は省略）。
- **Bsky 側の反映**: Bsky はピン留めが1件までのため、`pinned_posts` の先頭（`pinned_at`
  最新）のみを `app.bsky.actor.profile` の `pinnedPost`（`com.atproto.repo.strongRef`）として
  `AtpCommitService::commit_profile` に渡す。対象ポストが ATP 上に存在しない
  （`at_uri`/`at_cid` が無い）場合は `pinnedPost` を送らない。
- **Fedi 側の反映**: Actor ドキュメントの `featured` フィールド
  （`https://{domain}/users/{username}/collections/featured`）が指す `OrderedCollection` を
  `seiran-federation-inbox` が都度動的生成する（Note オブジェクトを `Create` でラップせず
  直接列挙、最大5件のためページングなし）。Add/Remove Activity のフォロワーへの配送は
  行わない（リモート側がプロフィール取得時に都度フェッチする経路で十分と判断、詳細は
  Doc3 §10）。

### 1.14 `lists` / `list_members` (リスト機能、#63)

Twitter/Mastodon の「リスト」に相当する機能。ユーザーごとに複数のリストを持て、
ローカル/リモートFedi/Bskyのアクターを混在してメンバー登録できる。公開リストは
Fediverse には AP Collection として、Bluesky には `app.bsky.graph.list`/`listitem`
として実際にPDSリポジトリへコミットし、双方のプロトコルからネイティブに閲覧できる
（Mastodon本家のリストは常に非公開だが、seiranはこの点を独自拡張している）。

```sql
CREATE TABLE lists (
    id BIGINT PRIMARY KEY,  -- generate_snowflake_id() で採番
    owner_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    is_public BOOLEAN NOT NULL DEFAULT false,

    -- 公開リストを app.bsky.graph.list として自前PDSにコミットした結果
    -- （非公開リスト、またはコミット未実施の間は NULL）。
    at_rkey VARCHAR(255),
    at_uri VARCHAR(2048) UNIQUE,
    at_cid VARCHAR(255),

    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (owner_actor_id, name)
);

CREATE INDEX idx_lists_owner ON lists(owner_actor_id);
CREATE INDEX idx_lists_public ON lists(owner_actor_id) WHERE is_public = true;

CREATE TABLE list_members (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    list_id BIGINT NOT NULL REFERENCES lists(id) ON DELETE CASCADE,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    added_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- app.bsky.graph.listitem コミット結果（対象が actor_type <> 'fedi' の場合のみ）。
    at_rkey VARCHAR(255),
    at_uri VARCHAR(2048) UNIQUE,
    UNIQUE (list_id, actor_id)
);

CREATE INDEX idx_list_members_list ON list_members(list_id, added_at DESC);
CREATE INDEX idx_list_members_actor ON list_members(actor_id);
```

- **可視性制限**: 公開リストをFediから見た場合は `actor_type <> 'bsky'`、Bskyから見た場合は
  `actor_type <> 'fedi'` でメンバーを絞り込む。ローカルアクターの `ap_uri` は常にNULL
  （AP Actor URIは `https://{domain}/users/{username}` として動的生成されるため）なので、
  「Fedi側から見えるか」の判定に `ap_uri IS NOT NULL` は使えない点に注意。
- **プロキシアクター（list-relay）**: 誰にもフォローされていないリモートFediユーザーの
  投稿を受信するため、seiranは `actor_type='local'`・`user_id=NULL` の仮想アクター
  `list-relay` を持つ（`seiran_common::system_actor::ensure_system_proxy_actor`が起動時に
  冪等生成し、`actor_id`を`site_settings`キー`system_proxy_actor_id`に記録）。AP署名は
  ローカルアクター共通のサーバー単一RSA鍵を流用するため専用鍵は不要。フォロー要否は
  **参照カウント方式**（`list_members`からの動的COUNT、`ListRepository::
  actor_referenced_by_any_list`）で判定し、`Job::ProxyFollowSync`がFollow/Undo Followを
  送信する。ユーザー名`list-relay`は一般ユーザーが登録できない予約名として`register()`が
  拒否する（`seiran_common::username`参照、§1.2の命名規則も参照）。
- **Bsky受信フィルタ**: `seiran-atp-repo`のJetstreamハンドラは、DIDが
  「ローカルユーザーにフォローされている」か「いずれかの`list_members`に含まれる」
  いずれかを満たす場合のみ投稿を保存する（無関係投稿の際限ない取り込みを防ぐ、
  過去に104万行まで膨張した事故の反省を踏襲）。
- **リストタイムライン**: `ListRepository::timeline`は`home_timeline`と同じ
  targets-LATERAL集約パターンを、対象を「自分+フォロー中」から`list_members`に
  差し替えて再利用する。
- **上限**: 1アクターあたりリスト数30個、1リストあたりメンバー数500人（提案値、
  `MAX_LISTS_PER_OWNER`/`MAX_MEMBERS_PER_LIST`定数）。
- 詳細なプロトコル仕様（AP Collectionのエンドポイント形状、ATP `app.bsky.graph.list`/
  `listitem`のレコード仕様）はDoc3 §14を参照。

---

## 2. データベース層での主要クエリ・ユースケース

### 2.1 リモートseiranユーザーの統合タイムライン取得
```sql
SELECT p.* FROM posts p
WHERE p.actor_id IN (
    101, -- フォローしているアクターのID (例: AP版)
    (SELECT seiran_pair_actor_id FROM actors WHERE id = 101) -- ペアリングされた対のアクターID (例: ATP版)
)
  AND p.deleted_at IS NULL
ORDER BY p.id DESC
LIMIT 30;
```

### 2.2 検索の未来掘り（`sinceId`）セッション消滅時フォールバック
```sql
SELECT p.* FROM posts p
WHERE p.id > :since_id 
  AND p.body ILIKE :query_pattern
  AND p.deleted_at IS NULL
ORDER BY p.id ASC
LIMIT 30;
```

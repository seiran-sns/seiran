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
    
    -- 表示用メタデータ
    username VARCHAR(255) NOT NULL, -- ユーザー名（@のあとの英数字）
    domain VARCHAR(255) NOT NULL,   -- インスタンスドメイン（例: mstdn.jp, bsky.social）
    display_name VARCHAR(255),
    avatar_url VARCHAR(2048),       -- リモートアクターはこちらを使用
    banner_url VARCHAR(2048),       -- リモートアクターはこちらを使用
    avatar_media_id BIGINT REFERENCES media_files(id) ON DELETE SET NULL, -- ローカルアクターのみ
    banner_media_id BIGINT REFERENCES media_files(id) ON DELETE SET NULL, -- ローカルアクターのみ
    bio TEXT,
    
    -- 相互マッピング用外部キーポインタ
    seiran_pair_actor_id BIGINT REFERENCES actors(id) ON DELETE SET NULL, -- リモートseiranの対の行
    bridge_real_actor_id BIGINT REFERENCES actors(id) ON DELETE SET NULL, -- ブリッジ時の「本尊」の行
    
    created_at TIMESTAMP WITH TIME ZONE NOT NULL,
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL
);

-- インデックス戦略
CREATE INDEX idx_actors_type ON actors(actor_type);
CREATE INDEX idx_actors_pair ON actors(seiran_pair_actor_id) WHERE seiran_pair_actor_id IS NOT NULL;
CREATE INDEX idx_actors_bridge ON actors(bridge_real_actor_id) WHERE bridge_real_actor_id IS NOT NULL;
CREATE INDEX idx_actors_user_id ON actors(user_id);                         -- 認証エンドポイントの find_local_by_user_id 高速化
CREATE INDEX idx_actors_username_domain ON actors(username, domain);        -- WebFinger / プロフィール検索 / フォロー解決高速化
```

### 1.3 `posts` (統一ポストテーブル)
宇宙の壁を超えて集約されるすべての投稿。未来補正タイムスタンプ付きのIDで完全に一次元ソートされる。編集競合を避けるため、更新系の動的データ（リアクション等）は別テーブルに完全分離する。

```sql
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
    
    -- 削除・変更管理
    deleted_at TIMESTAMP WITH TIME ZONE DEFAULT NULL, -- 論理削除フラグ
    atp_tombstone_cid VARCHAR(255) DEFAULT NULL,     -- ATPリポジトリ内での削除証明用CID
    
    created_at TIMESTAMP WITH TIME ZONE NOT NULL,
    inserted_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL
);

-- インデックス戦略
CREATE INDEX idx_posts_actor_id ON posts(actor_id, id DESC) WHERE deleted_at IS NULL; -- recent_by_actor / context_* のインデックススキャン化
CREATE INDEX idx_posts_reply_to ON posts(reply_to_post_id) WHERE reply_to_post_id IS NOT NULL;
CREATE INDEX idx_posts_repost_of ON posts(repost_of_post_id) WHERE repost_of_post_id IS NOT NULL;
CREATE INDEX idx_posts_quote_of ON posts(quote_of_post_id) WHERE quote_of_post_id IS NOT NULL;
CREATE INDEX idx_posts_parent_original ON posts(parent_original_post_id) WHERE parent_original_post_id IS NOT NULL;
CREATE INDEX idx_posts_seiran_uuid ON posts(seiran_post_uuid) WHERE seiran_post_uuid IS NOT NULL;
CREATE INDEX idx_posts_timeline_active ON posts(id) WHERE deleted_at IS NULL;

-- 全文検索用インデックス（pg_trgmを利用したTrigram部分一致インデックス）
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE INDEX idx_posts_body_trgm ON posts USING gin (body gin_trgm_ops);
```

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
    
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL,
    
    -- 同一ユーザーによる同一ポストへの同一リアクション重複を防止
    UNIQUE(post_id, actor_id, content)
);

-- 右ペインの「リアクション集計・打ったメンツ一覧」を爆速化するための複合インデックス
CREATE INDEX idx_reactions_post_content ON reactions(post_id, content);
CREATE INDEX idx_reactions_actor_id ON reactions(actor_id);
```

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
CREATE INDEX idx_follows_target_follower ON follows(target_actor_id, follower_actor_id) WHERE status = 'accepted'; -- ホームTL / AP配送フォロワー取得高速化
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

アップロードされた画像のメタデータ。オーナー概念を持たない。重複排除キーは `(sha256, blurhash)` の複合一致。

```sql
CREATE TABLE media_files (
    id                   BIGINT PRIMARY KEY,
    storage_provider_id  BIGINT NOT NULL REFERENCES storage_providers(id),
    sha256               CHAR(64) NOT NULL,
    blurhash             VARCHAR(100) NOT NULL,
    size                 BIGINT NOT NULL,             -- バイト数
    width                INT NOT NULL,
    height               INT NOT NULL,
    mime_type            VARCHAR(50) NOT NULL DEFAULT 'image/webp',
    storage_key          VARCHAR(1024) NOT NULL,
    uploaded_by_actor_id BIGINT REFERENCES actors(id) ON DELETE SET NULL, -- GC・監査用（オーナーシップではない）
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (sha256, blurhash),
    UNIQUE (storage_provider_id, storage_key)
);

CREATE INDEX idx_media_files_sha256 ON media_files(sha256);
```

**重複排除フロー:**
1. アップロード受信 → WebP 変換・リサイズ
2. 変換後バイト列の SHA-256 と blurhash を計算（ハッシュは WebP 変換後に算出）
3. `(sha256, blurhash)` が DB に存在 → 既存 `media_file_id` を返す（PUT スキップ）
4. SHA-256 一致・blurhash 不一致 → 別画像として通常保存
5. 両方不在 → ストレージ選択 → PUT → INSERT

**画像処理仕様（変換後の保存寸法）:**

| 用途 | 変換処理 | 保存寸法 |
|---|---|---|
| アバター | 中央正方形クロップ → WebP | 600 × 600 px |
| バナー | fit: inside → WebP | 横最大 2048 × 縦最大 768 px |
| カスタム絵文字 | fit: inside → WebP | 横最大 384 × 縦最大 64 px |
| ポスト添付 | fit: inside → WebP | 長辺最大 2048 px |

- サムネイルは生成しない。オリジナルは保持しない。WebP 1種のみ保存する。
- アニメーション画像はアニメ WebP として保持する。

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

### 1.9 `post_attachments` (ポスト添付画像)

```sql
CREATE TABLE post_attachments (
    post_id        BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    media_file_id  BIGINT NOT NULL REFERENCES media_files(id),
    position       SMALLINT NOT NULL,   -- 表示順（0 始まり）
    alt_text       TEXT,
    PRIMARY KEY (post_id, position)
);

CREATE INDEX idx_post_attachments_media ON post_attachments(media_file_id);
```

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
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

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

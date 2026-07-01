# Doc 1. データベース・スキーマ計画書 (Database Schema Blueprint)

本ドキュメントは、`seiran` システムにおけるエンティティ間の関係性、データ型、および分散ネットワーク特有のインデックス戦略を定義する。
* **想定RDB:** PostgreSQL 15以降

---

## 1. テーブル定義 ＆ 物理データモデル

### 1.1 `users` (ローカル認証・アカウント管理)
Auth0等の外部認証プロバイダの識別子と、内部の「魂（Identity）」をマッピングする。

```sql
CREATE TABLE users (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    auth0_sub VARCHAR(255) UNIQUE NOT NULL, -- 例: 'google-oauth2|1029...'
    email VARCHAR(255) UNIQUE NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL
);
```

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
    avatar_url VARCHAR(2048),
    banner_url VARCHAR(2048),
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
CREATE INDEX idx_posts_actor_id ON posts(actor_id);
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
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL,
    
    UNIQUE(follower_actor_id, target_actor_id)
);

CREATE INDEX idx_follows_follower ON follows(follower_actor_id);
CREATE INDEX idx_follows_target ON follows(target_actor_id);
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

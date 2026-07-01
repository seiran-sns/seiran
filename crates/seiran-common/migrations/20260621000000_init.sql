-- 20260621000000_init.sql
-- seiran初期マイグレーション

-- 1. users (ローカル認証・アカウント管理)
CREATE TABLE users (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    auth0_sub VARCHAR(255) UNIQUE NOT NULL, -- 例: 'google-oauth2|1029...'
    email VARCHAR(255) UNIQUE NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL
);

-- 2. actors (統一アクターテーブル)
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
    ap_uri VARCHAR(2048) UNIQUE, -- ActivityPubのActor URI
    at_did VARCHAR(255) UNIQUE,  -- AT ProtocolのDID
    
    -- 表示用メタデータ
    username VARCHAR(255) NOT NULL, -- ユーザー名
    domain VARCHAR(255) NOT NULL,   -- インスタンスドメイン
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

-- インデックス戦略 (actors)
CREATE INDEX idx_actors_type ON actors(actor_type);
CREATE INDEX idx_actors_pair ON actors(seiran_pair_actor_id) WHERE seiran_pair_actor_id IS NOT NULL;
CREATE INDEX idx_actors_bridge ON actors(bridge_real_actor_id) WHERE bridge_real_actor_id IS NOT NULL;

-- 3. posts (統一ポストテーブル)
CREATE TABLE posts (
    id BIGINT PRIMARY KEY, -- 補正タイムスタンプ内包の統一ポストID（ソート主軸）
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    
    -- コンテンツ本体
    body TEXT NOT NULL,
    
    -- 3大リレーション
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
    metadata JSONB DEFAULT '{}'::jsonb NOT NULL, 
    
    -- 削除・変更管理
    deleted_at TIMESTAMP WITH TIME ZONE DEFAULT NULL, -- 論理削除フラグ
    atp_tombstone_cid VARCHAR(255) DEFAULT NULL,     -- ATPリポジトリ内での削除証明用CID
    
    created_at TIMESTAMP WITH TIME ZONE NOT NULL,
    inserted_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL
);

-- インデックス戦略 (posts)
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

-- 4. reactions (リアクション独立テーブル)
CREATE TABLE reactions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    post_id BIGINT NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    
    reaction_type VARCHAR(50) NOT NULL DEFAULT 'emoji', -- 例: 'emoji', 'like'
    content VARCHAR(255) NOT NULL,                      -- 例: "👍", ":custom_emoji:"
    
    -- 外部宇宙での削除（Undo）追跡用ID
    ap_activity_id VARCHAR(2048) UNIQUE,
    at_uri VARCHAR(2048) UNIQUE,
    
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL,
    
    -- 同一ユーザーによる同一ポストへの同一リアクション重複を防止
    UNIQUE(post_id, actor_id, content)
);

CREATE INDEX idx_reactions_post_content ON reactions(post_id, content);
CREATE INDEX idx_reactions_actor_id ON reactions(actor_id);

-- 5. follows (フォロー関係テーブル)
CREATE TABLE follows (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    follower_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    target_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP NOT NULL,
    
    UNIQUE(follower_actor_id, target_actor_id)
);

CREATE INDEX idx_follows_follower ON follows(follower_actor_id);
CREATE INDEX idx_follows_target ON follows(target_actor_id);

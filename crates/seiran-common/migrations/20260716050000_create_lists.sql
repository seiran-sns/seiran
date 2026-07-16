-- リスト機能（#63）。
-- ユーザーごとに複数のリストを持て、ローカル/Fedi/Bskyのアクターを混在してメンバー登録できる。
-- 公開リストの可視性はアプリケーション層で actor_type により絞り込む
-- （Fediから見た場合は actor_type <> 'bsky'、Bskyから見た場合は actor_type <> 'fedi'）。
-- id は posts/actors と同じく generate_snowflake_id() でアプリケーション側が採番する。
CREATE TABLE lists (
    id BIGINT PRIMARY KEY,
    owner_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    name VARCHAR(100) NOT NULL,
    is_public BOOLEAN NOT NULL DEFAULT false,

    -- 公開リストを app.bsky.graph.list として自前PDSリポジトリに書き込んだ結果
    -- （非公開リスト、またはコミット未実施の間は NULL のまま）。
    at_rkey VARCHAR(255),
    at_uri VARCHAR(2048) UNIQUE,
    at_cid VARCHAR(255),

    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (owner_actor_id, name)
);

CREATE INDEX idx_lists_owner ON lists(owner_actor_id);
CREATE INDEX idx_lists_public ON lists(owner_actor_id) WHERE is_public = true;

-- リストメンバー（ローカル/Fedi/Bskyのactorを混在登録できる）。
-- 参照カウント方式のプロキシフォロー可否判定・Jetstream受信フィルタの両方で
-- idx_list_members_actor（actor_id 側索引）を使う。
CREATE TABLE list_members (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    list_id BIGINT NOT NULL REFERENCES lists(id) ON DELETE CASCADE,
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    added_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- 公開リストの app.bsky.graph.listitem レコード（対象が actor_type <> 'fedi' の場合のみ）。
    at_rkey VARCHAR(255),
    at_uri VARCHAR(2048) UNIQUE,

    UNIQUE (list_id, actor_id)
);

CREATE INDEX idx_list_members_list ON list_members(list_id, added_at DESC);
CREATE INDEX idx_list_members_actor ON list_members(actor_id);

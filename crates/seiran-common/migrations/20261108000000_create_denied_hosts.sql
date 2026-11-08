-- 拒否サーバー（管理画面「サーバーブロック」）。登録したホストとそのサブドメインに属する
-- リモートアクターは、ローカルからフォロー（またはリストに追加）されていない限り投稿を
-- 受け取らず、宛先本人がフォローしていない限り働きかけを受け付けない。詳細は
-- docs/architecture.md「拒否サーバー」。
CREATE TABLE denied_hosts (
    host TEXT PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ATP アカウントの所属サーバー（DID ドキュメントの #atproto_pds のホスト）。Bsky 行は
-- `domain` が空なので、拒否サーバーの判定にはこちらを使う。checked_at は再解決の目安。
ALTER TABLE actors
    ADD COLUMN pds_host TEXT,
    ADD COLUMN pds_host_checked_at TIMESTAMPTZ;

-- host が拒否サーバー（またはそのサブドメイン）か。
CREATE FUNCTION host_is_denied(h TEXT)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT h IS NOT NULL AND h <> '' AND EXISTS (
        SELECT 1 FROM denied_hosts d
        WHERE h = d.host OR right(h, length(d.host) + 1) = '.' || d.host
    )
$$;

-- アクターが拒否サーバーに属するか。AP 側の `domain` と ATP 側の `pds_host` のどちらかが
-- 該当すれば真（リモート seiran はどちらの経路で届いても同じ扱いにするため）。
CREATE FUNCTION actor_is_denied(p_actor_id BIGINT)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM actors a
        WHERE a.id = p_actor_id AND a.actor_type <> 'local'
          AND (host_is_denied(a.domain) OR host_is_denied(a.pds_host))
    )
$$;

-- ローカルの誰かがフォロー（承認済み）しているか、いずれかのリストのメンバーか。
-- 拒否サーバーのアクターの投稿を受け取る条件。
CREATE FUNCTION actor_is_locally_followed(p_actor_id BIGINT)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM follows f
        JOIN actors follower ON follower.id = f.follower_actor_id
        WHERE f.target_actor_id = p_actor_id AND f.status = 'accepted'
          AND follower.actor_type = 'local'
    ) OR EXISTS (SELECT 1 FROM list_members lm WHERE lm.actor_id = p_actor_id)
$$;

-- グローバルタイムライン・ハッシュタグタイムライン（とそのストリーミング）に出さない
-- アクターか。quarantine ロールのローカルユーザーと、拒否サーバーのアクター。
CREATE FUNCTION actor_is_kept_off_public_feeds(p_actor_id BIGINT)
RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1 FROM actors a
        JOIN users u ON u.id = a.user_id
        WHERE a.id = p_actor_id AND u.role = 'quarantine'
    ) OR actor_is_denied(p_actor_id)
$$;

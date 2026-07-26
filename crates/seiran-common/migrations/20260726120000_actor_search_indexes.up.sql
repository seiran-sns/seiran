-- リスト・DMの部分一致検索用。表示名と全ハンドル表記を1つの文字列へまとめ、
-- pg_bigmのGIN式インデックスで検索する。ローカル判定は環境依存のドメイン値を
-- migrationへ焼き込まず、同義であるactor_type='local'を使う。
CREATE INDEX idx_actors_search_bigm ON actors USING gin (
    LOWER(
        COALESCE(display_name, '')
        || E'\n@' || username
        || CASE WHEN domain <> '' THEN '@' || domain ELSE '' END
        || CASE WHEN actor_type = 'local'
                THEN E'\n@' || username || '.' || domain ELSE '' END
    ) gin_bigm_ops
);

-- 投稿欄の@サジェストは部分一致不要なので、text_pattern_opsのB-tree式インデックスで
-- 前方一致する。2本目はローカルユーザーのBsky形式ハンドル専用。
CREATE INDEX idx_actors_handle_prefix ON actors (
    LOWER(username || CASE WHEN domain <> '' THEN '@' || domain ELSE '' END) text_pattern_ops
);
CREATE INDEX idx_actors_local_bsky_handle_prefix ON actors (
    LOWER(CASE WHEN actor_type = 'local'
               THEN E'\n@' || username || '.' || domain END) text_pattern_ops
);

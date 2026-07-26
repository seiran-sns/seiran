-- LIKE検索をpg_bigm（2-gram GINインデックス）で高速化する(#97)。
-- pg_bigmはLIKE演算子のみ最適化対象でILIKEには対応しないため、アプリ側の検索クエリは
-- LOWER(col) LIKE LOWER(pattern) 形式に統一し、インデックスもLOWER()適用後の式に張る。
DROP INDEX IF EXISTS idx_posts_body_trgm;
DROP EXTENSION IF EXISTS pg_trgm;

CREATE EXTENSION IF NOT EXISTS pg_bigm;

-- 投稿本文検索（GET /api/notes/search）
CREATE INDEX idx_posts_body_bigm ON posts USING gin (LOWER(body) gin_bigm_ops);

-- アクター検索（GET /api/actors/search、リストメンバー追加のサジェスト）
CREATE INDEX idx_actors_username_bigm ON actors USING gin (LOWER(username) gin_bigm_ops);
CREATE INDEX idx_actors_display_name_bigm ON actors USING gin (LOWER(display_name) gin_bigm_ops);
CREATE INDEX idx_actors_acct_bigm ON actors USING gin (LOWER(username || '@' || domain) gin_bigm_ops);

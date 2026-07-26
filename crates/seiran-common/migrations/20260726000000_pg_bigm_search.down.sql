DROP INDEX IF EXISTS idx_posts_body_bigm;
DROP EXTENSION IF EXISTS pg_bigm;

CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE INDEX idx_posts_body_trgm ON posts USING gin (body gin_trgm_ops);

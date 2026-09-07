-- ユーザー凍結をローカル・リモート共通の actors.suspended_at へ一本化する。
-- 従来の users.suspended_at はローカル専用かつログイン・投稿・表示のどこからも参照されておらず
-- 実効性が無かった（#凍結リモート対応）。actors 側に持たせることで、AP/ATP のリモートアクターも
-- 同じ列・同じ enforcement（extract_auth・actor_is_hidden_for_viewer 等）に乗せられる。
ALTER TABLE actors ADD COLUMN suspended_at TIMESTAMPTZ NULL;

UPDATE actors a
SET suspended_at = u.suspended_at
FROM users u
WHERE a.user_id = u.id AND u.suspended_at IS NOT NULL;

ALTER TABLE users DROP COLUMN suspended_at;

CREATE INDEX idx_actors_suspended_at ON actors (suspended_at) WHERE suspended_at IS NOT NULL;

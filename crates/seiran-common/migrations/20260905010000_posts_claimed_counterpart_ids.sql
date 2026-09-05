-- 20260905000000_posts_claimed_counterpart_ids.sql
-- 他seiranサーバー間の投稿マージ（相互申告方式、docs/protocols.md 5節・#237）用。
-- ATP起源で先着した行が自己申告する相手プロトコル(AP)側の真正ID、
-- AP起源で先着した行が自己申告する相手プロトコル(ATP)側の真正IDを保持する。
-- 既存行自身のこの自己申告が、後から届いた実IDを指し返している場合にのみマージが成立する。
ALTER TABLE posts ADD COLUMN claimed_ap_object_id VARCHAR(2048);
ALTER TABLE posts ADD COLUMN claimed_at_uri VARCHAR(2048);

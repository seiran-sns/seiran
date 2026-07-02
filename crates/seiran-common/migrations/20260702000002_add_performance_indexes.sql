-- パフォーマンス改善インデックス追加
-- 参照: docs/improvement_db_performance.md

-- [高-1] actors.user_id インデックス
-- 全認証エンドポイントで毎回実行される find_local_by_user_id のシーケンシャルスキャンを解消
CREATE INDEX IF NOT EXISTS idx_actors_user_id ON actors(user_id);

-- [高-2] actors.(username, domain) 複合インデックス
-- WebFinger / プロフィール検索 / フォロー解決で使用される find_by_username_domain を高速化
CREATE INDEX IF NOT EXISTS idx_actors_username_domain ON actors(username, domain);

-- [高-3] follows.(target_actor_id, follower_actor_id) カバリングインデックス（status='accepted' 部分インデックス）
-- ホームTL サブクエリおよび AP 配送でのフォロワー取得を高速化
CREATE INDEX IF NOT EXISTS idx_follows_target_follower ON follows(target_actor_id, follower_actor_id) WHERE status = 'accepted';

-- [高-4] posts.(actor_id, id DESC) 複合部分インデックス（削除済み除外）
-- recent_by_actor / context_before / context_after の ORDER BY id を index scan で処理するため
-- 既存の単純インデックス idx_posts_actor_id を置き換え
DROP INDEX IF EXISTS idx_posts_actor_id;
CREATE INDEX IF NOT EXISTS idx_posts_actor_id ON posts(actor_id, id DESC) WHERE deleted_at IS NULL;

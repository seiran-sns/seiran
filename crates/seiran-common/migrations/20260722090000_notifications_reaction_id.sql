-- リアクション通知の重複排除用トークン。ローカルリアクション作成時に採番した reactions.id を
-- ここに保存し、そのリアクションを ATP へコミットする際に app.bsky.feed.like レコードの
-- 非標準拡張フィールド（seiranReactionId）としても同じ値を載せる。ATP 側から自分自身の
-- firehose 経由でこの Like が戻ってきた際、同一の reaction_id を持つ通知を UNIQUE 制約で
-- 弾けるようにし、「ローカル即時通知」と「firehose 再受信通知」の二重発生を防ぐ。
-- source_uri と異なり決定的なキーではなく「同一リアクション実体」の識別が目的のため、
-- 絵文字を変えて連投する遊び（同一 (post_id, actor_id) でも都度別の reactions.id）は妨げない。
ALTER TABLE notifications ADD COLUMN reaction_id BIGINT;
CREATE UNIQUE INDEX idx_notifications_reaction_id ON notifications (reaction_id) WHERE reaction_id IS NOT NULL;

-- follows(status='accepted')に限定した部分インデックス2本を削除する（#215）。
--
-- Fediverseのユースケースでは未承認フォローは常に少数（ほとんどが承認済みか未フォロー）で、
-- サーバー規模が大きくなってもこの比率は変わらない見込みのため、status='accepted'限定の
-- カバリング/複合インデックスは将来的にも有効に働く見込みが薄いと判断した。
-- また実測（followsテーブル427行）ではプランナが一貫してSeq Scanを選び、
-- 追加以降 idx_scan=0 のままだった（docs/code_audit_2026-08-05.md P-2、#215参照）。
DROP INDEX IF EXISTS idx_follows_target_follower;
DROP INDEX IF EXISTS idx_follows_follower_accepted;

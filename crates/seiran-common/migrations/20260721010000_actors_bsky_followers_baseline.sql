-- Bsky側フォロワー検知ポーリング（getFollowers）用のアクター単位「初回シード済み」マーカー。
-- NULL＝未シード。機能導入時、既に実フォロー済みの全Bskyフォロワーが初回ポーリングで
-- 一斉に「新規フォロー」と誤検出され通知が大量発生するのを防ぐために使う。
ALTER TABLE actors ADD COLUMN bsky_followers_baseline_done_at TIMESTAMPTZ;

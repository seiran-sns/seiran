-- 20260828000000_add_actors_hide_from_algorithmic_recommendations.sql
-- Bskyの app.bsky.actor.contentVisibilityDeclaration（hideFromAlgorithmicRecommendations）に
-- 対応するローカルキャッシュ。true の場合、Bsky側のDiscoverフィード等のアルゴリズム
-- レコメンドから除外するよう要求するアカウントレベルの宣言をPDSへコミットする。
ALTER TABLE actors ADD COLUMN hide_from_algorithmic_recommendations BOOLEAN NOT NULL DEFAULT false;

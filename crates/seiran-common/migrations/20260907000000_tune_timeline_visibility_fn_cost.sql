-- ホームタイムライン（フォロー数の多いユーザーで顕著）の初期表示・追加読み込みが
-- 1.3〜1.5秒かかっていた問題への対応。
--
-- home_timeline / social_timeline の LATERAL 内で post_is_visible_to /
-- post_reply_target_followed / repost_is_muted_for_viewer を呼んでいるが、
-- これらSQL関数のデフォルトコスト(100)をプランナが過小評価し、既存の
-- idx_posts_actor_id (actor_id, id DESC) への Index Scan ではなく
-- Bitmap Heap Scan を選択していた。Bitmap Heap Scan は LIMIT を pushdown
-- できないため、投稿数の多いフォロイー1人につき、可視性フィルタのために
-- そのアクターの投稿をほぼ全件ヒープから読み直してからソートする動きになり、
-- フォロー数が多いユーザーほど致命的に遅くなっていた（実測 1.6秒、フォロー
-- 746人・最大投稿数3,549件のフォロイーを含むケース）。
--
-- 関数の実コスト（内部でEXISTSサブクエリを伴う）に近い値へCOSTを引き上げると、
-- プランナが自発的にIndex Scanを選ぶようになり実測165ms程度まで改善する。
ALTER FUNCTION post_is_visible_to(bigint, bigint, text, bigint, boolean) COST 10000;
ALTER FUNCTION post_reply_target_followed(bigint, bigint) COST 10000;
ALTER FUNCTION repost_is_muted_for_viewer(bigint, bigint) COST 10000;

-- 上記のCOST引き上げにより本クエリの推定コストがJIT閾値(jit_above_cost等の
-- デフォルト10万〜50万)を超えてしまい、JITコンパイルのオーバーヘッド
-- （実測約290ms）がかえって上乗せされる副作用を確認した。このDBはOLTP用途
-- （タイムライン・投稿等の短小クエリが大半）でJITの恩恵が薄いため、JIT自体を
-- 無効化する。
DO $$
BEGIN
    EXECUTE format('ALTER DATABASE %I SET jit = off', current_database());
END $$;

-- 可視性判定の SQL 関数には既定のコストを使い、JIT も既定の設定で動かす。
-- ホーム/ソーシャル/リストの候補抽出はこれらの関数を呼ばず（判定をクエリに展開する）、
-- 対象アクター数によって LATERAL 方式と時間窓方式を切り替えて推定コストを
-- jit_above_cost 未満に保つ（crates/seiran-common/src/repository/feed.rs）。
ALTER FUNCTION post_is_visible_to(bigint, bigint, text, bigint, boolean) COST 100;
ALTER FUNCTION post_reply_target_followed(bigint, bigint) COST 100;
ALTER FUNCTION repost_is_muted_for_viewer(bigint, bigint) COST 100;

DO $$
BEGIN
    EXECUTE format('ALTER DATABASE %I RESET jit', current_database());
END $$;

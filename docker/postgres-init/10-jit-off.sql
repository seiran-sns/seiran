-- タイムライン系クエリは SQL 関数に COST 10000 を付けており（マイグレーション
-- 20260907000000）、プランの推定コストが JIT 閾値を大きく超える。JIT は実行の
-- たびにコンパイルし直すため、有効のままだとホームタイムラインが毎回約180ms遅くなる。
-- ALTER DATABASE ... SET は pg_dump に含まれず、バックアップからの復元では失われるので、
-- 空のデータディレクトリから初期化されるとき（新規構築・リストア先DB）にここで設定する。
DO $$
BEGIN
    EXECUTE format('ALTER DATABASE %I SET jit = off', current_database());
END $$;

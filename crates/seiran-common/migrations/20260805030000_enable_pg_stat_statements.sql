-- クエリ別の実行時間・呼び出し回数を計測するための拡張（docs/code_audit_2026-08-05.md P-9）。
-- shared_preload_librariesにpg_stat_statementsを積んだ上でこの拡張を有効化することで、
-- pg_stat_statementsビューからクエリ別の統計が見えるようになる。
-- 注意: shared_preload_librariesの変更はpostmasterの再起動が必要なため、
-- コンテナ再作成前はCREATE EXTENSIONが成功してもビューは空のまま（モジュール未ロード）。
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;

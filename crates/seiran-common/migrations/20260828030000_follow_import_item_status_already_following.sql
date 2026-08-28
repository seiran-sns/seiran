-- 20260828030000_follow_import_item_status_already_following.sql
-- フォローインポートの進捗表示で「成功」件数が実際のフォロー成立数（followsテーブル）と
-- 食い違う不具合への対応。execute_followは、既にフォロー関係が存在するターゲットへの
-- 再フォロー試行もエラーにせず成功扱いにしていたため、進捗の「成功」件数が実態より
-- 多く見えていた。これを区別するため、既存関係だった場合専用のステータスを追加する。

ALTER TYPE follow_import_item_status ADD VALUE 'already_following';

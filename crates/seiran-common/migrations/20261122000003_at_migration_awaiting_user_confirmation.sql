-- DID 転入の最終確認。転入元の認証とリポジトリの取得が済んだところで止め、元のサーバーでの
-- ログインが無効化されること・ハンドルが変わることを利用者に確かめてもらってから、確認コードの
-- 送付（requestPlcOperationSignature）へ進む。利用者の入力待ちの一つとして、48時間の打ち切りと
-- 案内メールの対象にする。
ALTER TYPE at_migration_status ADD VALUE IF NOT EXISTS 'awaiting_user_confirmation' AFTER 'fetching_repo';

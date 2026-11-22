-- DID 転入の最終確認。確認コードの入力を受けて転入元に PLC 操作へ署名させた（署名済みの操作は
-- 後から送っても有効）ところで止め、元のサーバーでのログインが無効化されること・ハンドルが
-- 変わることを利用者に確かめてもらってから、PLC 送信（不可逆）へ進む。利用者の入力待ちの一つ
-- として、48時間の打ち切りと案内メールの対象にする。
ALTER TYPE at_migration_status ADD VALUE IF NOT EXISTS 'awaiting_user_confirmation' AFTER 'awaiting_plc_token';

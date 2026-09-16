-- 転出元API対応（Phase A前提）: アカウント単位のPLCローテーションキーを追加。
-- NULLは「まだアカウント単位鍵へ移行していない」（バックフィル待ち、またはリモート
-- キャッシュ行）を意味する。既存のat_signing_key_pemと同じ保護レベル（平文PEM）。
ALTER TABLE actors ADD COLUMN at_rotation_key_pem TEXT;

-- DID転出済み状態。is_suspended（凍結）・転入フローのmigration_statusゲートとも異なる
-- 第三の状態で、書き込み系操作のみを禁止する（タイムライン等の読み取りは通常通り）。
-- submitPlcOperation成功時、またはdeactivateAccount呼び出し時にセットする。
ALTER TABLE actors ADD COLUMN did_moved_out_at TIMESTAMPTZ;

-- 既存DID転入フロー: 転入元PDSの既存ローテーションキーを維持する設計を廃止し、
-- 転入完了時にseiranが新規発行する専用ローテーションキーのみをDIDの鍵とする
-- （転入元PDS運営者に恒久的な支配権を残さないための方針転換）。
-- new_signing_key_pemと同じ理由（PLC提出直後、アカウント作成失敗時にも鍵を失わないため）
-- でmark_plc_submitted時点で先行して保存する。
ALTER TABLE at_migration_requests ADD COLUMN new_rotation_key_pem TEXT;

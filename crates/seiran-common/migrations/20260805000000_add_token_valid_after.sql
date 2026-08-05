-- パスワード変更・リセット・2FA設定変更時に更新し、それ以前に発行されたJWTを
-- 一括で失効させるための基準時刻（docs/code_audit_2026-08-05.md S-2）。
-- NULL は「制約なし（token_valid_after チェックを行わない）」を意味する。
ALTER TABLE users ADD COLUMN token_valid_after TIMESTAMPTZ DEFAULT NULL;

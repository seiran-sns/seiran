-- 利用規約同意の証跡。新規登録時にチェックボックスへ同意した場合のみ1行作られる
-- （利用規約テキストが空、またはチェックを入れなかった場合は行自体を作らない）。
CREATE TABLE terms_of_service_agreements (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 同意した時点で site_settings.terms_of_service_text に設定されていた文面そのもの。
    -- 後から管理者が文面を変更してもこの行の内容は変わらない（「何に同意したか」の証跡のため）。
    agreed_text TEXT NOT NULL,
    agreed_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_terms_of_service_agreements_user_id ON terms_of_service_agreements (user_id);

-- 既存DID転入フロー（docs/account_migration.md）は start 時点でチェックボックスに同意しても
-- users/actors 行が確定するのはずっと後（submit_plc_token）のため、同意文面を一旦ここへ
-- 保持し、アカウント確定時に terms_of_service_agreements へコピーする。
ALTER TABLE at_migration_requests ADD COLUMN agreed_tos_text TEXT;

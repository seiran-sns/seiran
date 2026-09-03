-- ローカルユーザーのプライバシー設定「フォロー承認制」（鍵アカウント）。
-- true の場合、フォローリクエストは即座に成立せず pending のまま
-- 本人の承認（accept）/拒否（reject）を待つ。
ALTER TABLE actors ADD COLUMN is_locked BOOLEAN NOT NULL DEFAULT false;

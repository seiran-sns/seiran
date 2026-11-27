-- Bluesky のプライバシー設定のうち、ATP リポジトリの self レコードで表す2つを actors に持つ。
-- - hide_from_unauthenticated: プロフィールの self-label `!no-unauthenticated`
--   （ログアウトしたユーザーに自分のアカウントを表示しないようアプリやサイトに求める）
-- - allow_activity_subscriptions: `app.bsky.notification.declaration` の `allowSubscriptions`
--   （他のユーザーにあなたの投稿の通知を許可する。レコードが無ければ followers が公式の既定値）
ALTER TABLE actors ADD COLUMN hide_from_unauthenticated BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE actors ADD COLUMN allow_activity_subscriptions TEXT NOT NULL DEFAULT 'followers'
    CHECK (allow_activity_subscriptions IN ('followers', 'mutuals', 'none'));

-- 既存ユーザーが bsky.app 等で書いた self レコードの内容は SQL で CBOR を読めないため、
-- 起動時バックフィル（seiran-api `backfill_atp_self_settings`）が読んで反映し、行を消す。
CREATE TABLE atp_self_settings_backfill (
    actor_id BIGINT PRIMARY KEY REFERENCES actors(id) ON DELETE CASCADE
);
INSERT INTO atp_self_settings_backfill (actor_id)
SELECT a.id
FROM actors a
WHERE a.actor_type = 'local'
  AND EXISTS (
    SELECT 1 FROM atp_records r
    WHERE r.actor_id = a.id
      AND r.rkey = 'self'
      AND r.collection IN (
        'app.bsky.actor.profile',
        'app.bsky.notification.declaration',
        'app.bsky.actor.contentVisibilityDeclaration'
      )
  );

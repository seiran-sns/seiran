-- 20260820010000_add_atp_preferences.sql
-- AT Protocol の app.bsky.actor.getPreferences/putPreferences 用。
-- クライアント設定・年齢確認(birthDate)等の不透明なJSON配列をそのまま保存・返却する
-- （ATPリポジトリのMSTには入らない、PDSのプライベートデータ）。

CREATE TABLE atp_preferences (
    actor_id BIGINT PRIMARY KEY REFERENCES actors(id) ON DELETE CASCADE,
    preferences JSONB NOT NULL DEFAULT '[]'::jsonb,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

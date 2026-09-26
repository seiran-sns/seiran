-- bio/profile_fields中のURL（Fedi/Bsky）を非同期解決した結果のキャッシュ。
-- 陽性（actor/postが見つかった）・陰性（見つからなかった/対象外）の両方を保存し、
-- 陰性は一定時間（アプリ側でchecked_atから判定、48時間目安）後に再調査対象にする。
CREATE TABLE link_resolutions (
    url               TEXT PRIMARY KEY,
    kind              TEXT NOT NULL, -- 'actor' | 'post' | 'none'
    resolved_actor_id BIGINT REFERENCES actors(id) ON DELETE SET NULL,
    resolved_post_id  BIGINT REFERENCES posts(id) ON DELETE SET NULL,
    checked_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

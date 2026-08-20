-- 20260820030000_backfill_birth_date_from_preferences.sql
-- app.bsky.actor.putPreferences経由で#personalDetailsPrefが保存されたものの、まだ
-- actors.birth_dateへ転記する処理が実装される前（2026-08-20 putPreferences初期実装時点）に
-- 設定された値をバックフィルする。以降はputPreferencesが直接actors.birth_dateへ同期するため、
-- atp_preferences側のpersonalDetailsPrefは不要（actors.birth_dateとの二重管理を避けるため除去）。

UPDATE actors a
SET birth_date = sub.birth_date
FROM (
    SELECT ap.actor_id,
           (elem->>'birthDate')::timestamptz::date AS birth_date
    FROM atp_preferences ap,
         jsonb_array_elements(ap.preferences) elem
    WHERE elem->>'$type' = 'app.bsky.actor.defs#personalDetailsPref'
      AND elem->>'birthDate' IS NOT NULL
) sub
WHERE a.id = sub.actor_id
  AND a.birth_date IS NULL;

UPDATE atp_preferences
SET preferences = (
    SELECT COALESCE(jsonb_agg(elem), '[]'::jsonb)
    FROM jsonb_array_elements(preferences) elem
    WHERE elem->>'$type' != 'app.bsky.actor.defs#personalDetailsPref'
)
WHERE EXISTS (
    SELECT 1 FROM jsonb_array_elements(preferences) elem
    WHERE elem->>'$type' = 'app.bsky.actor.defs#personalDetailsPref'
);

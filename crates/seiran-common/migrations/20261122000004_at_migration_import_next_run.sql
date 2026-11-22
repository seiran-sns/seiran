-- DID 転入の取り込みジョブ（自分を積み直しながら進む）が、次に動く予定の時刻。予定を過ぎても
-- 動いていなければ連鎖が途切れたとみなし、定期処理が積み直す（無停止更新の間に取り込みの状態を
-- 知らない版のワーカーがジョブを拾って捨てた場合など）。ジョブは続きを積むときだけ書く。
-- NULL はまだ一度も動いていないことで、その間は状態が変わった時刻（updated_at）から数える。
ALTER TABLE at_migration_requests ADD COLUMN import_next_run_at TIMESTAMPTZ;

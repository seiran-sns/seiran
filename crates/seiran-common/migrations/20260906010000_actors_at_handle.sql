-- remote_seiranアクターのBskyハンドル（ATPハンドル、`user.pds-domain`形式）を保持する列。
--
-- remote_seiran（結婚成立済み）行の`username`/`domain`はFedi側の値のまま固定され（マージ後の
-- 上書きガード、`upsert_remote_bsky`/`discover_bsky_actor_once`参照）、AT Protocol側の
-- ハンドル文字列はこれまで永続化されずマージ時に破棄されていた。プロフィール画面でFedi ID・
-- Bsky IDの両方を表示するには別列が必要なため追加する。`username`と異なりremote_seiranでも
-- 常に最新値へ更新する（bskyアクターの`username`列と同じ扱い）。
ALTER TABLE actors ADD COLUMN at_handle TEXT;

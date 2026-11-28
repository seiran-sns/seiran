-- Bluesky 側の行を、DID 側の宣言レコード（`org.seiran.actor.declaration`）を確かめずに作った
-- ことの印。検索結果のように多数の初見のアクターを一度に取り込む経路は、宣言の取得
-- （1人あたり約1秒）を待たずに行を作り、`Job::ActorMetadataResolve` が後から取る。
-- 印のある行は「行が無いときだけ取る」経路（`DeclarationLookup::WhenNew`）でも取り直す。
-- 既定値つきで足すので表の書き直しは起きず、既存の行は確かめ済みの扱いになる。
ALTER TABLE actors ADD COLUMN atp_declaration_pending BOOLEAN NOT NULL DEFAULT false;

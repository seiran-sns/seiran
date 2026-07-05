-- 退会機能（#29 Phase A）: アクターの退会日時カラムを追加する。
-- withdrawn_at が NULL でない場合、そのアクターは退会済みとして扱われる。
ALTER TABLE actors ADD COLUMN withdrawn_at TIMESTAMP WITH TIME ZONE;

CREATE INDEX idx_actors_withdrawn_at ON actors(withdrawn_at) WHERE withdrawn_at IS NOT NULL;

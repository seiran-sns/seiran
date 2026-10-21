-- ユーザーが「今後は表示しない」を選んだ確認ガイドのキー一覧（設定画面「おせっかいなガイドの
-- 非表示」）。キーの許容値は seiran_common::guides::HIDEABLE_GUIDES。端末をまたいで共有する
-- ため localStorage ではなく DB に持つ。
ALTER TABLE users ADD COLUMN hidden_guides TEXT[] NOT NULL DEFAULT '{}';

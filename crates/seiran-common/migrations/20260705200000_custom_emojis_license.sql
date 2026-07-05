-- 絵文字インポート機能（#50）: Misskey エクスポートに含まれるライセンス情報を保持するカラムを追加する。
ALTER TABLE custom_emojis ADD COLUMN license TEXT;

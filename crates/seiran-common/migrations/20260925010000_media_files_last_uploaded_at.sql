-- media_files の孤立ファイルGC（run_media_gc）は created_at のみを生存判定に使っていたため、
-- 「古いファイルのsha256が重複アップロードでID再利用された直後、GCがcreated_atだけを見て
-- 誤って削除する」レースが存在した（既存の投稿添付から外れて孤児化したファイルを後日
-- 誰かが再アップロードし、post_attachments登録前にGCが割り込むケース）。
-- 生存判定を「最後にアップロード（新規保存 or 重複としてヒット）された時刻」に切り替える。
ALTER TABLE media_files ADD COLUMN last_uploaded_at TIMESTAMP WITH TIME ZONE;
UPDATE media_files SET last_uploaded_at = created_at WHERE last_uploaded_at IS NULL;
ALTER TABLE media_files ALTER COLUMN last_uploaded_at SET NOT NULL;
ALTER TABLE media_files ALTER COLUMN last_uploaded_at SET DEFAULT now();

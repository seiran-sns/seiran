-- Bsky公式動画パイプライン（app.bsky.video.uploadVideo）結合のための状態カラム。
-- uploadVideo呼び出し時に返るjobId、getJobStatus完了時のblob CID、
-- 処理状況（NULL=未対象 / 'pending' / 'ready' / 'failed'）を保持する。
ALTER TABLE media_files
    ADD COLUMN bsky_video_job_id TEXT,
    ADD COLUMN bsky_video_cid TEXT,
    ADD COLUMN bsky_video_status TEXT;

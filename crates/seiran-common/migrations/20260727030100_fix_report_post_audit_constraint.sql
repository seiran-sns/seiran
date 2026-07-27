-- 投稿の物理削除時にも ON DELETE SET NULL で通報監査履歴を保持できるようにする。
ALTER TABLE reports DROP CONSTRAINT reports_subject_consistent;
ALTER TABLE reports ADD CONSTRAINT reports_subject_consistent CHECK (
    (subject_type = 'actor' AND subject_post_id IS NULL)
    OR subject_type = 'post'
);

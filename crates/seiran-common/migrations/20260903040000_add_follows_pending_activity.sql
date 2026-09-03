-- ロックアカウント宛てにFediverse(AP)から届いたFollowアクティビティを、承認/拒否時に
-- Accept/Rejectとして送り返せるよう一時的に保持する（承認はユーザー操作まで非同期に
-- 遅延するため、受信時点のリクエストボディを永続化しておく必要がある）。
ALTER TABLE follows ADD COLUMN pending_follow_activity JSONB;

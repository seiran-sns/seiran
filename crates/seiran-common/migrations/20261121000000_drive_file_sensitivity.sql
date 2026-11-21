-- Misskey 互換 API でドライブのファイルに付けた NSFW 指定（`drive/files/create`・
-- `drive/files/update` の `isSensitive`）の預け先。Misskey の `notes/create` は添付ごとの
-- 指定を持たないので、投稿の作成で添付ごとの指定が送られなかったときにここを読み、
-- `post_attachments.is_sensitive` へ写す。表示・配送はこの表を見ない（指定を変えても
-- 投稿済みの添付は変わらない）。`media_files` は同じ内容のファイルを利用者をまたいで
-- 共有するため、指定は利用者ごとに持つ。
CREATE TABLE drive_file_sensitivity (
    actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    media_file_id BIGINT NOT NULL REFERENCES media_files(id) ON DELETE CASCADE,
    PRIMARY KEY (actor_id, media_file_id)
);

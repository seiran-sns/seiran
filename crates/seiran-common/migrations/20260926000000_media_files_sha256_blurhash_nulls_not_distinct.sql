-- media_files の重複排除（アップロード時の「無ければ作る」）をアトミックな
-- INSERT ... ON CONFLICT (sha256, blurhash) DO UPDATE として実装するには、
-- (sha256, blurhash) の一意制約が ON CONFLICT の対象として機能する必要がある。
-- デフォルトのUNIQUE制約はNULL同士を「等しくない」とみなすため、blurhash=NULL
-- （ATP uploadBlob経由の画像・動画・音声すべてが該当）の行同士はそもそも重複と
-- 判定されず、同時アップロードで同一内容の行が複数できてしまう抜け道になっていた。
ALTER TABLE media_files DROP CONSTRAINT media_files_sha256_blurhash_key;
ALTER TABLE media_files ADD CONSTRAINT media_files_sha256_blurhash_key
    UNIQUE NULLS NOT DISTINCT (sha256, blurhash);

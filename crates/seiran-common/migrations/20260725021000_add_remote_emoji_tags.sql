-- リモート絵文字がAP Emoji tagで公開している別名・タグ・キーワードを保持し、
-- 管理画面でshortcodeと同様に部分一致検索できるようにする（#73）。
ALTER TABLE remote_emojis
    ADD COLUMN tags TEXT[] NOT NULL DEFAULT '{}';

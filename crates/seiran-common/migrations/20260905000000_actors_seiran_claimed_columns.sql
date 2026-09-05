-- リモートseiranアクターの相互申告マージ（#236）用。
-- 自分が「相手プロトコルではこの識別子だ」と自己申告しているが、まだ相互一致で
-- 確認できていない値を保持する。確認が取れたら ap_uri/at_did 本体に確定させ、
-- こちらは NULL に戻す。詳細は docs/protocols.md 11節参照。
ALTER TABLE actors
    ADD COLUMN claimed_ap_uri VARCHAR(2048),
    ADD COLUMN claimed_at_did VARCHAR(255);

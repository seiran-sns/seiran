-- リモートサーバー表示のサーバーアイコン用。`<link rel="icon">`（無ければ`/favicon.ico`）を
-- 取得できた場合のみ非NULL。取得できなければフロントエンドは🌐絵文字にフォールバックする。
ALTER TABLE remote_instance_meta ADD COLUMN icon_url VARCHAR(1024);

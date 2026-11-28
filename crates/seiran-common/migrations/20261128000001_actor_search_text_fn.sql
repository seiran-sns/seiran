-- ユーザー検索（Misskey 互換 users/search・Mastodon 互換 accounts/search）の照合文字列。
-- 式を関数1か所にまとめ、索引（次のマイグレーション）とクエリの両方がこれを呼ぶ。
-- 式を索引とクエリに別々に書くと、少しでも食い違ったときに索引が黙って使われなくなる。
-- 区切りの改行は、隣り合う項目の末尾と先頭をつなげた語に一致させないため。

-- 表示名と各ハンドル表記（`@user@host`、ローカルは Bsky 形式の `@user.domain` も、
-- ATP ハンドルが Fedi のユーザー名と違えば `@at_handle` も）。並び順で「名前・ハンドルに
-- 一致した人」を bio だけに一致した人より先にするのに使う。
CREATE OR REPLACE FUNCTION actor_handle_search_text(
    display_name TEXT, username TEXT, domain TEXT, actor_type actor_type_enum, at_handle TEXT
) RETURNS TEXT
LANGUAGE sql IMMUTABLE PARALLEL SAFE
RETURN LOWER(
    COALESCE(display_name, '')
    || E'\n@' || username
    || CASE WHEN domain <> '' THEN '@' || domain ELSE '' END
    || CASE WHEN actor_type = 'local' THEN E'\n@' || username || '.' || domain ELSE '' END
    || CASE WHEN at_handle IS NOT NULL AND at_handle <> username
            THEN E'\n@' || at_handle ELSE '' END
);

-- 上に自己紹介（bio）を足した、検索で照合する文字列。
CREATE OR REPLACE FUNCTION actor_search_text(
    display_name TEXT, username TEXT, domain TEXT, actor_type actor_type_enum, at_handle TEXT,
    bio TEXT
) RETURNS TEXT
LANGUAGE sql IMMUTABLE PARALLEL SAFE
RETURN actor_handle_search_text(display_name, username, domain, actor_type, at_handle)
    || E'\n' || LOWER(COALESCE(bio, ''));

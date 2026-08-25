-- oEmbed embed機能（YouTube/Spotify/Apple Music/SoundCloud/Vimeo）の許可ドメイン初期値。
-- 新規サーバーだけでなく、このマイグレーションが流れる既存サーバーにも同じ初期値を与える
-- （管理画面で未編集の状態＝キー未作成の状態なので、このマイグレーションが唯一の初期化経路）。
--
-- 各行は「domain」または「domain,oembed_endpoint」。後者はHTMLページに
-- oEmbed discoveryタグを載せていないが、oEmbedエンドポイント自体は提供しているサイト
-- （Vimeo）向けの救済で、`?url=<対象URL>&format=json`付きで直接そのエンドポイントを叩く。
INSERT INTO site_settings (key, value, updated_at)
VALUES (
    'oembed_allowed_domains',
    E'youtube.com\nopen.spotify.com\nmusic.apple.com\nsoundcloud.com\nvimeo.com,https://vimeo.com/api/oembed.json',
    NOW()
)
ON CONFLICT (key) DO NOTHING;

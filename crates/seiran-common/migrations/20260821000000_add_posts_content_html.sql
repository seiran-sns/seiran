-- リモートFedi投稿の元AP Note.contentをサニタイズ済みHTMLとして保持する（seiran Web UIでの
-- <blockquote>/<ruby>等の構造保持表示用）。ローカル投稿・Bsky投稿・移行前の既存行はNULLのまま
-- （バックフィル不可。元の生HTMLを保存していないため）。
ALTER TABLE posts ADD COLUMN content_html TEXT;

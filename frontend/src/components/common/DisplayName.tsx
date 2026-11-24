import EmojiText from "../note/EmojiText";

export interface DisplayNameProps {
  /** 表示名。空・未設定なら `fallback` を出す。 */
  name?: string | null;
  /** 表示名が無いときに出す文字列（通常はユーザー名・ハンドル）。 */
  fallback: string;
  /** 表示名中の `:shortcode:` → 画像URL（API応答の `emojis`）。 */
  emojis?: Record<string, string> | null;
}

/**
 * ユーザーの表示名を描画する共通コンポーネント。カスタム絵文字のショートコードと Unicode 絵文字を
 * 画像に展開する。表示名を出す箇所はすべてこれを通す（素の文字列で出すと、その画面だけ
 * `:shortcode:` が文字のまま見える）。ラップ要素を持たないので、呼び出し側の要素のスタイルが効く。
 */
export default function DisplayName({ name, fallback, emojis }: DisplayNameProps) {
  return <EmojiText text={name || fallback} emojis={emojis ?? undefined} />;
}

import { parse as parseTwemoji } from "@twemoji/parser";
import { describe, expect, it } from "vitest";
import { findTwemojiMatches } from "./twemoji";

/**
 * Unicode で Emoji_Presentation=No（単独では文字として表示）かつ肌の色を変えられる10文字。
 * `@twemoji/parser` 17.0.2 はこれらを `text-default,diversity` という種別に移したが、その種別の
 * 正規表現（`textDefaultDiversityRegex`）を生成テンプレート `regex.js.mustache` に差し込み忘れて
 * いるため、FE0F 付きでも肌色付きでも絵文字として認識しない。残った FE0F だけが url の空の絵文字
 * として返り、本文に壊れた画像が出ていた（seiran.org の投稿 117681113767761869 の「☝️」）。
 * 上流の issue: https://github.com/jdecked/twemoji-parser/issues/16
 */
const TEXT_DEFAULT_DIVERSITY = [
  "261d", // ☝ up pointing index
  "26f7", // ⛷ skier
  "26f9", // ⛹ person bouncing ball
  "270c", // ✌ victory hand
  "270d", // ✍ writing hand
  "1f3cb", // 🏋 person lifting weights
  "1f3cc", // 🏌 person golfing
  "1f574", // 🕴 person in suit levitating
  "1f575", // 🕵 detective
  "1f590", // 🖐 hand with fingers splayed
];

const fromCodepoints = (...cps: string[]) => String.fromCodePoint(...cps.map((cp) => parseInt(cp, 16)));

// frontend/package.json の範囲は ">=17.0.0 <17.0.2 || >17.0.2 <18.0.0"。壊れている 17.0.2 だけを
// 外し、#16 が直った版が出れば取り込めるようにしてある。新しい版を取り込んだときにこのテストが
// 落ちたら、その版もまだ直っていないので、範囲からその版も外して 17.0.1 に戻すこと。
const BROKEN_PARSER_HINT =
  "@twemoji/parser が jdecked/twemoji-parser#16 のバグを含む版になっている（17.0.2 で混入）。" +
  "17.0.1 なら通る。新しい版を取り込んで落ちたのなら、その版も #16 が直っていない。" +
  "frontend/package.json の範囲からその版も外し（例: \">=17.0.0 <17.0.2 || >17.0.3 <18.0.0\"）、" +
  "package-lock.json を 17.0.1 に戻すこと。";

describe("@twemoji/parser の版（jdecked/twemoji-parser#16 を含まないこと）", () => {
  it.each(TEXT_DEFAULT_DIVERSITY)("U+%s に FE0F を付けると1つの絵文字として認識する", (cp) => {
    const text = fromCodepoints(cp, "fe0f");
    const matches = parseTwemoji(text, { buildUrl: (codepoints) => codepoints });
    expect(
      matches.map((m) => ({ url: m.url, indices: m.indices })),
      BROKEN_PARSER_HINT
    ).toEqual([{ url: cp, indices: [0, text.length] }]);
  });

  it.each(TEXT_DEFAULT_DIVERSITY)("U+%s に肌の色を付けると1つの絵文字として認識する", (cp) => {
    const text = fromCodepoints(cp, "1f3fd");
    const matches = parseTwemoji(text, { buildUrl: (codepoints) => codepoints });
    expect(
      matches.map((m) => ({ url: m.url, indices: m.indices })),
      BROKEN_PARSER_HINT
    ).toEqual([{ url: `${cp}-1f3fd`, indices: [0, text.length] }]);
  });
});

describe("findTwemojiMatches", () => {
  it("投稿 117681113767761869 の本文の絵文字をすべて画像にする", () => {
    const matches = findTwemojiMatches("問題発生 ☝️😆 どっちでもええやん🔁");
    expect(matches.map((m) => m.url)).toEqual([
      "/twemoji/261d.svg",
      "/twemoji/1f606.svg",
      "/twemoji/1f501.svg",
    ]);
  });

  // parser の正規表現は末尾に単独の FE0F に当たる分岐を持つため、絵文字でない文字の直後の FE0F は
  // どの版でも url の空の絵文字として返る。そのまま <img src=""> にすると壊れた画像になる。
  it.each(["←\u{fe0f}", "a\u{fe0f}", "\u{fe0f}"])("単独の FE0F（%j）は画像にしない", (text) => {
    expect(findTwemojiMatches(text)).toEqual([]);
  });
});

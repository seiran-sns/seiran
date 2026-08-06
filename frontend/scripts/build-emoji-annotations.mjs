// emojibase-data（node_modules）の各言語 data.json は hexcode/group/order/skins 等
// 検索インデックス（src/lib/emojiAnnotations.ts）で使わないフィールドまで含み、1言語あたり
// 700〜800kB（生JSON）ある。emoji/label/tagsだけを抜き出した軽量JSONを
// src/generated/emoji-annotations/ へ生成し、そちらをインポートさせることでダウンロード
// サイズを縮める（docs/code_audit_2026-08-05.md P-7）。node_modules由来の生成物なので
// git管理はせずpostinstallで都度生成する。
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

const LANGUAGES = ["en", "ja", "zh", "ko", "es", "de", "fr"];
const DEST_DIR = path.resolve(import.meta.dirname, "../src/generated/emoji-annotations");

async function buildLanguage(lang) {
  const { default: entries } = await import(`emojibase-data/${lang}/data.json`, {
    with: { type: "json" },
  });
  const trimmed = entries
    .filter((e) => e.emoji)
    .map((e) => (e.tags && e.tags.length > 0 ? { emoji: e.emoji, label: e.label, tags: e.tags } : { emoji: e.emoji, label: e.label }));
  await writeFile(path.join(DEST_DIR, `${lang}.json`), JSON.stringify(trimmed));
}

async function main() {
  await mkdir(DEST_DIR, { recursive: true });
  await Promise.all(LANGUAGES.map(buildLanguage));
  console.log(`[build-emoji-annotations] ${LANGUAGES.length} 言語分の軽量アノテーションを ${DEST_DIR} へ生成しました`);
}

main().catch((e) => {
  console.error("[build-emoji-annotations] 失敗:", e);
  process.exit(1);
});

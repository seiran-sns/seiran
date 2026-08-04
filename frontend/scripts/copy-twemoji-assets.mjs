// @twemoji/svg（node_modules）のSVGアセットを、Viteのpublic/経由でセルフホスト配信するために
// public/twemoji/ へコピーする。node_modules由来の生成物なのでgit管理はせずpostinstallで都度生成する。
import { mkdir, readdir, copyFile } from "node:fs/promises";
import path from "node:path";

const SRC_DIR = path.resolve(import.meta.dirname, "../node_modules/@twemoji/svg");
const DEST_DIR = path.resolve(import.meta.dirname, "../public/twemoji");

async function main() {
  await mkdir(DEST_DIR, { recursive: true });
  const entries = await readdir(SRC_DIR);
  const svgFiles = entries.filter((f) => f.endsWith(".svg"));
  await Promise.all(svgFiles.map((f) => copyFile(path.join(SRC_DIR, f), path.join(DEST_DIR, f))));
  console.log(`[copy-twemoji-assets] ${svgFiles.length} 個のSVGを ${DEST_DIR} へコピーしました`);
}

main().catch((e) => {
  console.error("[copy-twemoji-assets] 失敗:", e);
  process.exit(1);
});

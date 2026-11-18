// 依存監査の結果から、人の対応が必要なものの一覧を作って標準出力へ出す（何も無ければ空）。
// .github/scripts/audit-notify.sh から呼ぶ。
//
//   node audit-report.mjs <dependabot-alerts.json> <dependabot-prs.json> <cargo-audit.json> <npm-audit.json>
//
// 対象:
//   - Dependabot のアラートを読めないこと（Dependabot alerts が無効・権限不足等）
//   - 開いたままの Dependabot アラート（修正版がまだ無い、最小限の更新では届かない、PR が人の
//     判断待ち）。作られてから GRACE_HOURS 以内のものは、自動の修正（PR→CI→マージ）が
//     進行中でありうるので除く。
//   - 自動マージが付いていない Dependabot の PR（メジャー更新・勧告に紐づかない更新）
//   - cargo audit の脆弱性、npm audit（フロントの本番依存）の high 以上のうち、上の
//     アラートと同じ勧告（GHSA）でないもの。GitHub の勧告データベースに載っていない勧告は
//     Dependabot が扱えないため。
import { readFileSync } from "node:fs";

const GRACE_HOURS = 2;
const [alertsPath, prsPath, cargoPath, npmPath] = process.argv.slice(2);
const read = (path) => JSON.parse(readFileSync(path, "utf8"));

const lines = [];
const seen = new Set();
const now = Date.now();

const alerts = read(alertsPath);
if (!Array.isArray(alerts)) {
  // 取得に失敗したときは audit.yml が {error} を書く
  lines.push(
    `・Dependabot のアラートを読めません（リポジトリ設定の Dependabot alerts が有効か確認）: ${String(alerts.error).trim()}`,
  );
}
for (const a of Array.isArray(alerts) ? alerts : []) {
  if (now - Date.parse(a.created_at) < GRACE_HOURS * 3600_000) continue;
  const adv = a.security_advisory;
  const vuln = a.security_vulnerability;
  seen.add(adv.ghsa_id);
  for (const id of adv.identifiers ?? []) seen.add(id.value);
  const patched = vuln.first_patched_version?.identifier ?? "修正版なし";
  lines.push(
    `・${vuln.package.ecosystem} ${vuln.package.name}: ${adv.summary}（${adv.severity}、${patched}）${a.html_url}`,
  );
}

for (const pr of read(prsPath)) {
  if (pr.autoMergeRequest) continue;
  lines.push(`・人の判断待ちの PR: ${pr.title} ${pr.url}`);
}

for (const v of read(cargoPath).vulnerabilities?.list ?? []) {
  const adv = v.advisory;
  if ([adv.id, ...(adv.aliases ?? [])].some((id) => seen.has(id))) continue;
  seen.add(adv.id);
  const patched = v.versions?.patched?.length ? v.versions.patched.join(" / ") : "修正版なし";
  lines.push(
    `・cargo ${v.package.name} ${v.package.version}: ${adv.title}（${patched}）https://rustsec.org/advisories/${adv.id}`,
  );
}

for (const [name, v] of Object.entries(read(npmPath).vulnerabilities ?? {})) {
  for (const via of v.via) {
    if (typeof via !== "object" || !["high", "critical"].includes(via.severity)) continue;
    const ghsa = via.url?.split("/").pop();
    if (seen.has(ghsa)) continue;
    seen.add(ghsa);
    lines.push(`・npm ${name}: ${via.title}（${via.severity}、${via.range}）${via.url}`);
  }
}

// DM の上限（Bluesky の相手で 1000 文字）に収める。あふれた分は件数だけ示す。
const LIMIT = 900;
const out = [];
let length = 0;
for (const [i, line] of lines.entries()) {
  if (length + line.length + 1 > LIMIT) {
    out.push(`ほか ${lines.length - i} 件`);
    break;
  }
  out.push(line);
  length += line.length + 1;
}
if (out.length) console.log(out.join("\n"));

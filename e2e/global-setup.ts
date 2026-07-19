// E2E専用Postgresを起動し、healthyになるまで待つ。マイグレーション自体は
// backend（seiran-server）起動時に自動実行される（crates/seiran-common/src/db.rs の
// run_migrations）ので、ここでは待つだけでよい。
import { execFileSync } from "node:child_process";
import path from "node:path";

const e2eDir = path.dirname(new URL(import.meta.url).pathname);

export default async function globalSetup() {
  execFileSync("docker", ["compose", "-f", "docker-compose.yml", "up", "-d", "db"], {
    cwd: e2eDir,
    stdio: "inherit",
  });

  const deadline = Date.now() + 60_000;
  for (;;) {
    const status = execFileSync(
      "docker",
      ["compose", "-f", "docker-compose.yml", "ps", "db", "--format", "{{.Health}}"],
      { cwd: e2eDir },
    )
      .toString()
      .trim();
    if (status === "healthy") return;
    if (Date.now() > deadline) {
      throw new Error(`E2E Postgres が healthy になりませんでした（最終ステータス: ${status}）`);
    }
    await new Promise((r) => setTimeout(r, 500));
  }
}

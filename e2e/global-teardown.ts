// E2E専用Postgresを破棄する（`-v` でデータも消し、次回実行を空の状態から始める）。
import { execFileSync } from "node:child_process";
import path from "node:path";

const e2eDir = path.dirname(new URL(import.meta.url).pathname);

export default async function globalTeardown() {
  execFileSync("docker", ["compose", "-f", "docker-compose.yml", "down", "-v"], {
    cwd: e2eDir,
    stdio: "inherit",
  });
}

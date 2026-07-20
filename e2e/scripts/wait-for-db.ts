// E2E専用Postgresを起動し、実際に接続できるようになるまで待ってから終了する。
//
// 【重要】Playwrightの実行順序は「webServer起動 → globalSetup」であり、
// globalSetupではない（直感に反するが実際にそう。node_modules/playwright/lib/runner/index.js
// の createGlobalSetupTasks を見ると ...createPluginSetupTasks(config2)（webServerはpluginとして
// ここで起動する）が globalSetups.map(...) より前に来る）。そのため「E2E DBを起動して
// 待つ」処理はglobalSetupではなく、backend webServerのcommand自体の前段として
// 実行しなければならない（playwright.config.tsの該当コメント参照）。
import { execFileSync } from "node:child_process";
import { Socket } from "node:net";
import path from "node:path";

const e2eDir = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const HOST_PORT = 5433;

function waitForTcp(port: number, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    function attempt() {
      const socket = new Socket();
      socket.once("connect", () => {
        socket.destroy();
        resolve();
      });
      socket.once("error", () => {
        socket.destroy();
        if (Date.now() > deadline) {
          reject(new Error(`ポート ${port} への接続待ちがタイムアウトしました`));
        } else {
          setTimeout(attempt, 300);
        }
      });
      socket.connect(port, "127.0.0.1");
    }
    attempt();
  });
}

async function main() {
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
    if (status === "healthy") break;
    if (Date.now() > deadline) {
      throw new Error(`E2E Postgres が healthy になりませんでした（最終ステータス: ${status}）`);
    }
    await new Promise((r) => setTimeout(r, 500));
  }

  // コンテナ内部のヘルスチェックが通った直後でも、ホスト側のポートフォワーディングが
  // まだ準備できていないことがあるため、ホスト側から実際にTCP接続できることまで確認する。
  await waitForTcp(HOST_PORT, 10_000);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});

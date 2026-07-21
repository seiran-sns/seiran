// E2E専用のポート番号定義。scripts/dev-up.sh のネイティブ開発サーバー（3000/5173）とは
// 意図的に別ポートを使い、E2E実行中も本番相当の開発サーバーを止めずに済むようにする。
// playwright.config.ts と tests/*.spec.ts の両方から参照するため、
// テストプロセス・webServer子プロセスのどちらからでも import できるようここに集約する。
export const BACKEND_PORT = Number(process.env.E2E_BACKEND_PORT ?? "3100");
export const FRONTEND_PORT = Number(process.env.E2E_FRONTEND_PORT ?? "5273");
export const PLC_STUB_PORT = Number(process.env.PLC_STUB_PORT ?? "2582");
export const APPVIEW_STUB_PORT = Number(process.env.APPVIEW_STUB_PORT ?? "2583");

export const BACKEND_URL = `http://localhost:${BACKEND_PORT}`;
export const FRONTEND_URL = `http://localhost:${FRONTEND_PORT}`;

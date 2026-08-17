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

// バックエンドに渡す LOCAL_DOMAIN env と同じ値（playwright.config.ts の backendEnv 参照）。
// ローカルアクターの AP actor URI は `https://{LOCAL_DOMAIN}/users/{username}`
// （https固定・ポート番号なし）で組み立てられ、BACKEND_URL（http://localhost:PORT）とは
// 別物。fixtures/stub-fedi-server.ts がリモートActor視点でローカルアクターを参照する際
// （Follow.object・Create.to/cc・Mention.href）はこちらを使うこと。BACKEND_URLを流用すると
// スキーム・ポートの不一致で seiran_common::ap::extract_local_username の自ドメイン判定に
// 一致せず、フォロー・メンション・DM宛先解決が黙って失敗する。
export const LOCAL_DOMAIN = "localhost";

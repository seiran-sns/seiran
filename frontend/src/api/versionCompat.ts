import { FRONTEND_MIN_PEER_VERSION, FRONTEND_VERSION } from "../version";
import { isVersionAtLeast } from "../utils/semver";

/**
 * バックエンドの`middleware::version_headers::attach`（`crates/seiran-api/src/middleware/version_headers.rs`）
 * が全APIレスポンスへ付与するヘッダー名。
 */
export const SERVER_VERSION_HEADER = "x-seiran-server-version";
export const SERVER_MIN_PEER_VERSION_HEADER = "x-seiran-server-min-peer-version";

type ReloadRequiredHandler = () => void;
let reloadRequiredHandler: ReloadRequiredHandler | null = null;
let dismissed = false;

/** `ReloadRequiredDialog`がマウント時に登録する。 */
export function setReloadRequiredHandler(handler: ReloadRequiredHandler | null) {
  reloadRequiredHandler = handler;
}

/**
 * ダイアログを閉じた後、同じ非互換状態のままリロードせずに使い続けても
 * 再表示しないようにする（リロードすればモジュール状態ごとリセットされ、
 * 再度チェックが働く）。
 */
export function dismissReloadRequired() {
  dismissed = true;
}

/**
 * `request()`/`uploadFormData()`（`core.ts`）から全レスポンスに対して呼ばれる。
 * 【フロントエンドのバージョン ≥ サーバーの対応対向最低バージョン】
 * 【サーバーのバージョン ≥ フロントエンドの対応対向最低バージョン】
 * のいずれかを満たさない場合、リロードを促すダイアログを一度だけ出す。
 */
export function checkVersionCompat(res: Response) {
  if (dismissed) return;
  const serverVersion = res.headers.get(SERVER_VERSION_HEADER);
  const serverMinPeerVersion = res.headers.get(SERVER_MIN_PEER_VERSION_HEADER);
  if (!serverVersion || !serverMinPeerVersion) return;
  const compatible =
    isVersionAtLeast(FRONTEND_VERSION, serverMinPeerVersion) &&
    isVersionAtLeast(serverVersion, FRONTEND_MIN_PEER_VERSION);
  if (!compatible) {
    reloadRequiredHandler?.();
  }
}

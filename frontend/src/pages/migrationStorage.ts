// 既存DID転入フロー（`docs/account_migration.md`）の進行状態をlocalStorageへ橋渡しする。
// `MigratePanel`（フォーム・進行状況表示の両方を担う）から参照される。
const STORAGE_KEY = "seiran_migration_request";

export interface StoredMigrationRequest {
  /** snowflake IDはJSの53bit整数精度を超えるため文字列のまま扱う。数値化しないこと。 */
  id: string;
  token: string;
}

export function loadStoredMigrationRequest(): StoredMigrationRequest | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as StoredMigrationRequest;
    if (typeof parsed.id === "string" && typeof parsed.token === "string") return parsed;
    return null;
  } catch {
    return null;
  }
}

export function storeMigrationRequest(req: StoredMigrationRequest) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(req));
}

export function clearStoredMigrationRequest() {
  localStorage.removeItem(STORAGE_KEY);
}

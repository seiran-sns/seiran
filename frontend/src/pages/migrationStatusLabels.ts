import type { TFunction } from "i18next";
import type { MigrationStatusResponse } from "../api/migration";

// 既存DID転入フロー（`docs/account_migration.md`）の状態→表示文言マッピング。
// ログイン前（`MigratePanel`）・ログイン後（`MigrationImportingPage`）の両方から使う。
const STATUS_LABEL_KEYS: Record<string, string> = {
  fetching_repo: "auth:migrationStatus.step.fetchingRepo",
  requesting_plc_signature: "auth:migrationStatus.step.requestingPlcSignature",
  awaiting_plc_token: "auth:migrationStatus.step.awaitingPlcToken",
  submitting_plc: "auth:migrationStatus.step.submittingPlc",
  importing_data: "auth:migrationStatus.step.importingData",
  deactivating_source: "auth:migrationStatus.step.deactivatingSource",
  completed: "auth:migrationStatus.step.completed",
  failed: "auth:migrationStatus.step.failed",
  failed_post_submit: "auth:migrationStatus.step.failedPostSubmit",
  abandoned: "auth:migrationStatus.step.abandoned",
};

export function migrationStepLabel(t: TFunction, statusData: MigrationStatusResponse | null): string {
  if (!statusData) return "";
  if (
    statusData.status === "importing_data" &&
    statusData.import_total != null &&
    statusData.import_done != null
  ) {
    return t("auth:migrationStatus.step.importingDataProgress", {
      done: statusData.import_done,
      total: statusData.import_total,
    });
  }
  const key = STATUS_LABEL_KEYS[statusData.status];
  return key ? t(key) : statusData.status;
}

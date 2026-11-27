import { mailLanguage, request, uploadFormData } from "./core";
import type { Edit } from "../lib/editDiff";
import type { AdminReport, AdminUser, AuthIpBlock, DeniedHostsResponse, CustomEmoji, EmojiImportStatus, FediverseRelay, RemoteEmoji, ReportComment, SiteSettings, StorageProvider, SuspendedActor } from "./types";

export const admin = {
  listReports() {
    return request<AdminReport[]>("GET", "/admin/reports");
  },
  /** 未処理の通報の件数（管理メニュー・通報タブのバッジ用）。 */
  openReportCount() {
    return request<{ count: number }>("GET", "/admin/reports/open-count");
  },
  closeReport(id: string) {
    return request<void>(
      "POST",
      `/admin/reports/${encodeURIComponent(id)}/close`,
    );
  },
  listReportComments(id: string) {
    return request<ReportComment[]>(
      "GET",
      `/admin/reports/${encodeURIComponent(id)}/comments`,
    );
  },
  addReportComment(id: string, body: string) {
    return request<ReportComment>(
      "POST",
      `/admin/reports/${encodeURIComponent(id)}/comments`,
      { body },
    );
  },
  deleteReportedPost(id: string) {
    return request<void>(
      "POST",
      `/admin/reports/${encodeURIComponent(id)}/delete-post`,
    );
  },
  suspendReportedUser(id: string) {
    return request<void>(
      "POST",
      `/admin/reports/${encodeURIComponent(id)}/suspend-user`,
    );
  },
  /** 通報対象のローカルユーザーを制限（silence）・隔離（quarantine）へ下げる。 */
  restrictReportedUser(id: string, role: "silence" | "quarantine") {
    return request<void>(
      "POST",
      `/admin/reports/${encodeURIComponent(id)}/restrict-user`,
      { role },
    );
  },
  forwardReport(id: string) {
    return request<void>(
      "POST",
      `/admin/reports/${encodeURIComponent(id)}/forward`,
    );
  },
  /** 無限スクロール用カーソル(afterId)・絞り込み(q)対応のユーザー一覧取得。 */
  listUsers(opts?: { q?: string; afterId?: string; limit?: number }) {
    const params = new URLSearchParams();
    if (opts?.q) params.set("q", opts.q);
    if (opts?.afterId) params.set("after_id", opts.afterId);
    params.set("limit", String(opts?.limit ?? 30));
    return request<AdminUser[]>("GET", `/admin/users?${params.toString()}`);
  },
  suspendUser(id: string) {
    return request<void>(
      "POST",
      `/admin/users/${encodeURIComponent(id)}/suspend`,
    );
  },
  unsuspendUser(id: string) {
    return request<void>(
      "POST",
      `/admin/users/${encodeURIComponent(id)}/unsuspend`,
    );
  },
  /** `role` は選ぶ前のロールからの差分（別の画面で変えられていたら `EDIT_CONFLICT`）。 */
  changeUserRole(id: string, role: Edit<string>) {
    return request<void>(
      "POST",
      `/admin/users/${encodeURIComponent(id)}/role`,
      { role },
    );
  },
  disableUserTotp(id: string) {
    return request<void>(
      "POST",
      `/admin/users/${encodeURIComponent(id)}/totp/disable`,
    );
  },

  /** 凍結済みアクター一覧（ローカル・リモート混在、無限スクロール用カーソル対応）。 */
  listSuspendedActors(opts?: { afterId?: string; limit?: number }) {
    const params = new URLSearchParams();
    if (opts?.afterId) params.set("after_id", opts.afterId);
    params.set("limit", String(opts?.limit ?? 30));
    return request<SuspendedActor[]>(
      "GET",
      `/admin/suspended-actors?${params.toString()}`,
    );
  },
  /** actor_id起点の凍結・凍結解除（ローカル・リモート共通）。 */
  suspendActor(id: string) {
    return request<void>("POST", `/admin/actors/${encodeURIComponent(id)}/suspend`);
  },
  /** ユーザー操作メニュー用の、アクターの凍結状態・ロール・操作者が付けられるロール。 */
  actorModeration(id: string) {
    return request<{ suspended: boolean; role: string | null; assignable_roles: string[] }>(
      "GET",
      `/admin/actors/${encodeURIComponent(id)}/moderation`,
    );
  },
  changeActorRole(id: string, role: string) {
    return request<void>("POST", `/admin/actors/${encodeURIComponent(id)}/role`, { role });
  },
  unsuspendActor(id: string) {
    return request<void>("POST", `/admin/actors/${encodeURIComponent(id)}/unsuspend`);
  },

  getSiteSettings() {
    return request<SiteSettings>("GET", "/admin/site-settings");
  },
  /**
   * 変えた項目だけを差分（`Edit`）で送る。書き込み専用の項目（`smtp_password`・
   * `turnstile_secret_key`・`cloudflare_api_token`）は画面が現在値を知らないので値だけ送る
   * （上書き）。どれか1つでも別の画面の変更と重なったら何も保存されず `EDIT_CONFLICT`。
   */
  updateSiteSettings(
    patch: { [K in Exclude<keyof SiteSettings, `${string}_set`>]?: Edit<string> } & {
      smtp_password?: string;
      turnstile_secret_key?: string;
      cloudflare_api_token?: string;
    },
  ) {
    return request<SiteSettings>("PATCH", "/admin/site-settings", patch);
  },
  /** 入力中の SMTP 設定で接続・認証まで試す（省略・空のパスワードは保存済みの値を使う）。 */
  testSmtpConnection(body: SmtpTestRequest) {
    return request<SmtpTestResult>("POST", "/admin/site-settings/smtp-test", body);
  },
  /** 入力中の SMTP 設定で `to` へテストメールを送る。 */
  sendSmtpTestEmail(body: SmtpTestRequest & { to: string }) {
    return request<SmtpTestResult>("POST", "/admin/site-settings/smtp-test-email", {
      ...body,
      language: mailLanguage(),
    });
  },

  /** 管理画面のウィジェットで得たトークンを Siteverify にかける（Secret Key 省略・空なら保存済みの値）。 */
  testTurnstile(body: { token: string; turnstile_secret_key?: string }) {
    return request<TurnstileTestResult>("POST", "/admin/site-settings/turnstile-test", body);
  },

  getDeniedHosts() {
    return request<DeniedHostsResponse>("GET", "/admin/denied-hosts");
  },
  /** textarea の内容の差分。別の画面の変更と行単位で 3-way マージされる。 */
  updateDeniedHosts(text: Edit<string>) {
    return request<DeniedHostsResponse>("PUT", "/admin/denied-hosts", { text });
  },
  listAuthIpBlocks() {
    return request<AuthIpBlock[]>("GET", "/admin/auth-ip-blocks");
  },
  unblockAuthIp(ip: string) {
    return request<void>(
      "DELETE",
      `/admin/auth-ip-blocks/${encodeURIComponent(ip)}`,
    );
  },

  listStorageProviders() {
    return request<StorageProvider[]>("GET", "/admin/storage-providers");
  },
  createStorageProvider(body: {
    name: string;
    endpoint: string;
    bucket: string;
    region?: string;
    access_key: string;
    secret_key: string;
    public_url: string;
    capacity_mb?: number | null;
  }) {
    return request<StorageProvider>("POST", "/admin/storage-providers", body);
  },
  /** 変えた項目だけを差分で送る。`secret_key`（書き込み専用）と `is_active`（切り替え）は値だけ。 */
  updateStorageProvider(
    id: number,
    patch: {
      name?: Edit<string>;
      endpoint?: Edit<string>;
      bucket?: Edit<string>;
      region?: Edit<string>;
      access_key?: Edit<string>;
      secret_key?: string;
      public_url?: Edit<string>;
      capacity_mb?: Edit<number | null>;
      is_active?: boolean;
    },
  ) {
    return request<StorageProvider>(
      "PATCH",
      `/admin/storage-providers/${id}`,
      patch,
    );
  },
  deleteStorageProvider(id: number) {
    return request<void>("DELETE", `/admin/storage-providers/${id}`);
  },

  listEmojis() {
    return request<CustomEmoji[]>("GET", "/admin/emojis");
  },
  createEmoji(body: {
    shortcode: string;
    media_file_id: string;
    category?: string;
    tags?: string[];
    license?: string;
  }) {
    return request<CustomEmoji>("POST", "/admin/emojis", body);
  },
  updateEmoji(
    id: string,
    body: { category?: Edit<string>; tags?: Edit<string[]>; license?: Edit<string> },
  ) {
    return request<CustomEmoji>(
      "PATCH",
      `/admin/emojis/${encodeURIComponent(id)}`,
      body,
    );
  },
  deleteEmoji(id: string) {
    return request<void>("DELETE", `/admin/emojis/${encodeURIComponent(id)}`);
  },
  importEmojis(file: File): Promise<{ requestId: string }> {
    const formData = new FormData();
    formData.append("file", file);
    return uploadFormData<{ requestId: string }>("/admin/emojis/import", formData);
  },
  getEmojiImportStatus() {
    return request<EmojiImportStatus>("GET", "/admin/emojis/import");
  },
  cancelEmojiImport() {
    return request<void>("POST", "/admin/emojis/import/cancel");
  },
  /** エクスポートZIPのダウンロードURL（短命チケット付き）を発行する。 */
  issueEmojiExportUrl() {
    return request<{ url: string }>("POST", "/admin/emojis/export/ticket");
  },
  listRemoteEmojis(keyword?: string) {
    const q = keyword?.trim()
      ? `?keyword=${encodeURIComponent(keyword.trim())}`
      : "";
    return request<RemoteEmoji[]>("GET", `/admin/emojis/remote${q}`);
  },
  importRemoteEmoji(body: {
    shortcode: string;
    image_url: string;
    category?: string;
    tags?: string[];
    license?: string;
  }) {
    return request<CustomEmoji>("POST", "/admin/emojis/remote/import", body);
  },
  listRelays() {
    return request<FediverseRelay[]>("GET", "/admin/relays");
  },
  createRelay(inbox_url: string) {
    return request<FediverseRelay>("POST", "/admin/relays", { inbox_url });
  },
  deleteRelay(id: string) {
    return request<void>("DELETE", `/admin/relays/${encodeURIComponent(id)}`);
  },
};

export interface SmtpTestRequest {
  smtp_host?: string;
  smtp_port?: string;
  smtp_username?: string;
  smtp_password?: string;
  smtp_from?: string;
}

/** 失敗も 200 で返る。`code` は画面で翻訳する種別、`detail` は SMTP サーバー・lettre のエラー文。 */
export interface SmtpTestResult {
  ok: boolean;
  code?: "SMTP_NOT_CONFIGURED" | "SMTP_INVALID_ADDRESS" | "SMTP_FAILED";
  detail?: string;
}

/** 失敗も 200 で返る。`detail` は Siteverify の error-codes か通信エラー文、`hostname` はウィジェットを解いたホスト名。 */
export interface TurnstileTestResult {
  ok: boolean;
  code?: "TURNSTILE_NOT_CONFIGURED" | "TURNSTILE_FAILED" | "TURNSTILE_UNREACHABLE";
  detail?: string;
  hostname?: string;
}

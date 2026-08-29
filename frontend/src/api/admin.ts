import { request, uploadFormData } from "./core";
import type { AdminReport, AdminUser, AuthIpBlock, CustomEmoji, EmojiImportJob, FediverseRelay, RemoteEmoji, ReportComment, SiteSettings, StorageProvider } from "./types";

export const admin = {
  listReports() {
    return request<AdminReport[]>("GET", "/admin/reports");
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
  changeUserRole(id: string, role: string) {
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

  getSiteSettings() {
    return request<SiteSettings>("GET", "/admin/site-settings");
  },
  updateSiteSettings(
    patch: Partial<{
      smtp_host: string;
      smtp_port: string;
      smtp_username: string;
      smtp_password: string;
      smtp_from: string;
      require_email_verification: string;
      site_name: string;
      site_color: string;
      site_icon_url: string;
      media_proxy_url: string;
      auth_bruteforce_window_minutes: string;
      auth_bruteforce_max_variants: string;
      auth_ip_block_window_minutes: string;
      auth_ip_block_threshold: string;
      auth_ip_block_duration_hours: string;
      turnstile_site_key: string;
      turnstile_secret_key: string;
      password_reset_max_active: string;
      account_creation_ip_window_minutes: string;
      account_creation_ip_max: string;
      post_rate_limit_window_minutes: string;
      post_rate_limit_max_user: string;
      post_rate_limit_max_moderator: string;
      follow_rate_limit_window_hours: string;
      follow_rate_limit_max_user: string;
      follow_rate_limit_max_moderator: string;
      list_max_count_user: string;
      list_max_count_moderator: string;
      list_member_max_user: string;
      list_member_max_moderator: string;
      search_rate_limit_window_minutes: string;
      search_rate_limit_max_user: string;
      search_rate_limit_max_moderator: string;
      oembed_allowed_domains: string;
    }>,
  ) {
    return request<SiteSettings>("PATCH", "/admin/site-settings", patch);
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
  updateStorageProvider(id: number, patch: Record<string, unknown>) {
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
    body: { category?: string; tags?: string[]; license?: string },
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
  importEmojis(file: File): Promise<EmojiImportJob> {
    const formData = new FormData();
    formData.append("file", file);
    return uploadFormData<EmojiImportJob>("/admin/emojis/import", formData);
  },
  getEmojiImportStatus(jobId: string) {
    return request<EmojiImportJob>(
      "GET",
      `/admin/emojis/import/${encodeURIComponent(jobId)}`,
    );
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

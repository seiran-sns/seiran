/** 管理画面にアクセスできる役割か（admin / moderator）。 */
export function isAdminRole(role: string | undefined): boolean {
  return role === "admin" || role === "moderator";
}

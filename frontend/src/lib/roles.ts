/** 管理画面の各トピック（タブ）。 */
export type AdminTopic = "users" | "siteSettings" | "storage" | "emojis" | "reports" | "relays";

const ALL_ADMIN_TOPICS: AdminTopic[] = [
  "users",
  "siteSettings",
  "storage",
  "emojis",
  "reports",
  "relays",
];

/**
 * ロールごとにアクセス可能な管理画面トピック（#179）。
 * 権限の強さ: admin > moderator > emoji-editor > user。
 * moderator・emoji-editor は現状「絵文字」トピックのみアクセス可能。
 */
const ROLE_ADMIN_TOPICS: Record<string, AdminTopic[]> = {
  admin: ALL_ADMIN_TOPICS,
  moderator: ["emojis"],
  "emoji-editor": ["emojis"],
};

/** role がアクセスできる管理画面トピックの一覧を返す（権限なしは空配列）。 */
export function getAdminTopics(role: string | undefined): AdminTopic[] {
  if (!role) return [];
  return ROLE_ADMIN_TOPICS[role] ?? [];
}

/** 管理画面に（いずれかのトピックだけでも）アクセスできる役割か。 */
export function canAccessAdminPage(role: string | undefined): boolean {
  return getAdminTopics(role).length > 0;
}

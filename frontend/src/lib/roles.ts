/** 管理画面の各トピック（タブ）。 */
export type AdminTopic =
  | "users"
  | "siteSettings"
  | "storage"
  | "emojis"
  | "reports"
  | "suspendedActors"
  | "relays"
  | "authIpBlocks";

const ALL_ADMIN_TOPICS: AdminTopic[] = [
  "users",
  "siteSettings",
  "storage",
  "emojis",
  "reports",
  "suspendedActors",
  "relays",
  "authIpBlocks",
];

/**
 * ロールごとにアクセス可能な管理画面トピック（#179）。
 * 権限の強さ: admin > moderator > emoji-editor > user。
 * moderator は調停者として「通報」対応（凍結・投稿削除・連合転送を含む）・
 * 「凍結済みユーザー」一覧（バックエンドの`report_moderator_only`と対応、#凍結リモート対応）・
 * 「絵文字」管理にアクセス可能。emoji-editor は「絵文字」トピックのみ。
 */
const ROLE_ADMIN_TOPICS: Record<string, AdminTopic[]> = {
  admin: ALL_ADMIN_TOPICS,
  moderator: ["reports", "suspendedActors", "emojis"],
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

import { createDeviceFlag } from "./deviceFlag";

/**
 * 落ち着いたアイコン（設定画面「表示」＞「この端末だけの設定」）。オンの間、メニュー・設定メニュー・
 * 投稿カードのボタンバー・投稿フォームのボタンバーのアイコンを、カラフルな Twemoji でなくモノクロの
 * 線画（`components/common/UiIcon.tsx`）で描く。賑やかさの好みは画面ごとに違いうるので端末に持つ。
 */
export const CALM_ICONS_STORAGE_KEY = "seiran_calm_icons";

const flag = createDeviceFlag(CALM_ICONS_STORAGE_KEY);

export const setCalmIcons = flag.set;
export const useCalmIcons = flag.use;

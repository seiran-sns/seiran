import { createDeviceFlag } from "./deviceFlag";

/**
 * ギガ節約モード（設定画面「表示」＞「この端末だけの設定」）。オンの間、投稿に添付された画像・動画を
 * クリックするまで読み込まない。通信量を気にするかは回線（端末）次第なので、アカウントでなく
 * この端末の `localStorage` に持つ。
 */
export const DATA_SAVER_STORAGE_KEY = "seiran_data_saver";

const flag = createDeviceFlag(DATA_SAVER_STORAGE_KEY);

export const setDataSaver = flag.set;
export const useDataSaver = flag.use;

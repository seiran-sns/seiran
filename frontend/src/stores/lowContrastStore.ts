import { createDeviceFlag } from "./deviceFlag";

/**
 * 低コントラストモード（設定画面「表示」＞「この端末だけの設定」）。オンの間 `<html data-contrast="low">`
 * にし、`index.css` のカラートークンを刺激の少ない値（白すぎない背景・強すぎない文字色・彩度を落とした
 * アクセント）に替える。まぶしさの感じ方は画面次第なので端末に持つ。ちらつきを防ぐため、React の
 * マウント前にも `index.html` のインラインスクリプトで同じ属性を付ける。
 */
export const LOW_CONTRAST_STORAGE_KEY = "seiran_low_contrast";

function apply(enabled: boolean) {
  if (enabled) document.documentElement.setAttribute("data-contrast", "low");
  else document.documentElement.removeAttribute("data-contrast");
}

const flag = createDeviceFlag(LOW_CONTRAST_STORAGE_KEY, apply);

export const setLowContrast = flag.set;
export const useLowContrast = flag.use;

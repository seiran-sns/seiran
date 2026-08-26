import { decode } from "blurhash";

const cache = new Map<string, string | null>();

/**
 * blurhash文字列を小さなcanvasにデコードしdata URLへ変換する。
 * 同じhashは初回だけデコードし、結果（失敗時はnull）をキャッシュして使い回す。
 */
export function blurhashToDataUrl(hash: string): string | null {
  if (cache.has(hash)) return cache.get(hash) ?? null;

  let url: string | null = null;
  try {
    const size = 8;
    const pixels = decode(hash, size, size);
    const canvas = document.createElement("canvas");
    canvas.width = size;
    canvas.height = size;
    const ctx = canvas.getContext("2d");
    if (ctx) {
      const imageData = ctx.createImageData(size, size);
      imageData.data.set(pixels);
      ctx.putImageData(imageData, 0, 0);
      url = canvas.toDataURL();
    }
  } catch {
    url = null;
  }
  cache.set(hash, url);
  return url;
}

// Node 25 以降はグローバルに組み込みの `localStorage`/`sessionStorage` を持つ
// （`--localstorage-file` 未指定時は `undefined`）。vitest の jsdom 環境は既存のグローバルを
// 上書きしないため、そのままではテスト内の `localStorage` が jsdom ではなく Node の未初期化の
// 値を指してしまう。jsdom 側の実装を明示的に割り当て、Node のバージョンに依存させない
// （CI の Node 20 では元から jsdom の実装が使われるため、この処理は実質何もしない）。
const jsdomWindow = (globalThis as { jsdom?: { window: Window } }).jsdom?.window;
if (jsdomWindow) {
  for (const key of ["localStorage", "sessionStorage"] as const) {
    Object.defineProperty(globalThis, key, {
      value: jsdomWindow[key],
      configurable: true,
      writable: true,
    });
  }
}

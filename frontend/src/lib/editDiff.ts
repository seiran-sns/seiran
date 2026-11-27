import { ApiError } from "../api/core";

/**
 * 設定画面・管理画面の保存で送る、1項目の差分。画面を開いた（最後に保存した）時点の値 `from`
 * と、保存したい値 `to`。サーバーはロック中の現在値にこれを当て、別の画面の変更と重なれば
 * 保存せず `EDIT_CONFLICT` を返す（`crates/seiran-common/src/edit_diff.rs`）。値の項目は
 * 「現在値が `from` か `to` なら `to`」、テキストの項目は `from` を共通祖先にした 3-way マージ。
 */
export interface Edit<T> {
  from: T;
  to: T;
}

/** 差分の形のリクエスト（各項目が `Edit` で、変えない項目は省く）。 */
export type EditsOf<T> = { [K in keyof T]?: Edit<T[K]> };

function sameValue(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  // 配列・オブジェクト（タグ・プロフィールの項目等）は中身で比べる
  return typeof a === "object" && typeof b === "object" && JSON.stringify(a) === JSON.stringify(b);
}

/** `base`（読み込んだ時点の値）から `current`（画面の値）へ変わった項目だけの差分。 */
export function diffEdits<T extends object>(base: T, current: T): EditsOf<T> {
  const edits: EditsOf<T> = {};
  for (const key of Object.keys(current) as (keyof T)[]) {
    if (!sameValue(base[key], current[key])) {
      edits[key] = { from: base[key], to: current[key] };
    }
  }
  return edits;
}

/** 別の画面からの変更と重なって保存できなかったか。 */
export function isEditConflict(error: unknown): boolean {
  return error instanceof ApiError && error.code === "EDIT_CONFLICT";
}

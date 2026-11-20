/**
 * "x.y.z" の数値部分だけを比べる。版の接尾辞（`-RC1`・`-dev`・フォークの `-some-fork`、
 * `+build` 等。`docs/architecture.md` 2.1節）は無視する。semver の pre-release の順序
 * （`1.0.0-RC1` < `1.0.0`）に従うと、フォークの `1.0.0-some-fork` が最低対向版 `1.0.0` を
 * 満たさないと判定されてしまうため。接尾辞を数値として読むと `NaN` になり比較が壊れる。
 */
export function compareVersions(a: string, b: string): number {
  const pa = numericParts(a);
  const pb = numericParts(b);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const na = pa[i] ?? 0;
    const nb = pb[i] ?? 0;
    if (na !== nb) return na - nb;
  }
  return 0;
}

function numericParts(version: string): number[] {
  return version
    .split(/[-+]/, 1)[0]
    .split(".")
    .map((part) => Number(part) || 0);
}

/** `version` が `minVersion` 以上かどうか。 */
export function isVersionAtLeast(version: string, minVersion: string): boolean {
  return compareVersions(version, minVersion) >= 0;
}

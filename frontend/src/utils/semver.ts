/**
 * "x.y.z"形式のみを前提にした単純なバージョン比較（pre-release/buildメタデータ等は非対応）。
 * seiranのバージョンは`x.y.z`のみで運用する前提のため、これで十分。
 */
export function compareVersions(a: string, b: string): number {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const na = pa[i] ?? 0;
    const nb = pb[i] ?? 0;
    if (na !== nb) return na - nb;
  }
  return 0;
}

/** `version` が `minVersion` 以上かどうか。 */
export function isVersionAtLeast(version: string, minVersion: string): boolean {
  return compareVersions(version, minVersion) >= 0;
}

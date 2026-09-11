import { describe, expect, it } from "vitest";
import { compareVersions, isVersionAtLeast } from "./semver";

describe("compareVersions", () => {
  it("compares major/minor/patch numerically, not lexically", () => {
    expect(compareVersions("0.10.0", "0.9.0")).toBeGreaterThan(0);
    expect(compareVersions("1.0.0", "0.9.9")).toBeGreaterThan(0);
    expect(compareVersions("0.2.0", "0.2.0")).toBe(0);
    expect(compareVersions("0.1.0", "0.2.0")).toBeLessThan(0);
  });

  it("treats a missing trailing segment as 0", () => {
    expect(compareVersions("1.0", "1.0.0")).toBe(0);
    expect(compareVersions("1.2", "1.2.1")).toBeLessThan(0);
  });
});

describe("isVersionAtLeast", () => {
  it("is true when equal or newer", () => {
    expect(isVersionAtLeast("0.2.0", "0.1.0")).toBe(true);
    expect(isVersionAtLeast("0.1.0", "0.1.0")).toBe(true);
  });

  it("is false when older", () => {
    expect(isVersionAtLeast("0.1.0", "0.2.0")).toBe(false);
  });
});

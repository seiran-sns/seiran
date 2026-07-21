import { describe, expect, it } from "vitest";
import { isAdminRole } from "./roles";

describe("isAdminRole", () => {
  it("admin を許可する", () => {
    expect(isAdminRole("admin")).toBe(true);
  });

  it("moderator を許可する", () => {
    expect(isAdminRole("moderator")).toBe(true);
  });

  it("一般ユーザーの role を拒否する", () => {
    expect(isAdminRole("user")).toBe(false);
  });

  it("undefined を拒否する", () => {
    expect(isAdminRole(undefined)).toBe(false);
  });
});

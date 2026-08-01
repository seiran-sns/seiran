import { describe, expect, it } from "vitest";
import { canAccessAdminPage, getAdminTopics } from "./roles";

describe("canAccessAdminPage", () => {
  it("admin を許可する", () => {
    expect(canAccessAdminPage("admin")).toBe(true);
  });

  it("moderator を許可する", () => {
    expect(canAccessAdminPage("moderator")).toBe(true);
  });

  it("emoji-editor を許可する（#179）", () => {
    expect(canAccessAdminPage("emoji-editor")).toBe(true);
  });

  it("一般ユーザーの role を拒否する", () => {
    expect(canAccessAdminPage("user")).toBe(false);
  });

  it("undefined を拒否する", () => {
    expect(canAccessAdminPage(undefined)).toBe(false);
  });
});

describe("getAdminTopics", () => {
  it("admin は全トピックにアクセスできる", () => {
    expect(getAdminTopics("admin")).toEqual([
      "users",
      "siteSettings",
      "storage",
      "emojis",
      "reports",
      "relays",
    ]);
  });

  it("moderator は絵文字トピックのみアクセスできる（#179）", () => {
    expect(getAdminTopics("moderator")).toEqual(["emojis"]);
  });

  it("emoji-editor は絵文字トピックのみアクセスできる（#179）", () => {
    expect(getAdminTopics("emoji-editor")).toEqual(["emojis"]);
  });

  it("user はどのトピックにもアクセスできない", () => {
    expect(getAdminTopics("user")).toEqual([]);
  });

  it("undefined はどのトピックにもアクセスできない", () => {
    expect(getAdminTopics(undefined)).toEqual([]);
  });
});

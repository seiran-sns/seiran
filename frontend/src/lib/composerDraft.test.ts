import { beforeEach, describe, expect, it } from "vitest";
import {
  clearComposerDraft,
  ComposerDraft,
  loadComposerDraft,
  saveComposerDraft,
} from "./composerDraft";

function makeDraft(text: string): ComposerDraft {
  return {
    text,
    attachments: [],
    deliverFedi: true,
    deliverBsky: true,
    visibility: "public",
    bskyEmbedChoice: null,
    pollEnabled: false,
    pollChoices: ["", ""],
    pollMultiple: false,
    pollExpiry: { kind: "none" },
    cwEnabled: false,
    cwGuide: "",
    linkCardUrls: [],
  };
}

beforeEach(() => {
  localStorage.clear();
});

describe("composerDraft: compose", () => {
  it("保存した下書きを同じユーザーIDで読み出せる", () => {
    saveComposerDraft({ mode: "compose", userId: 1 }, makeDraft("書きかけ"));
    expect(loadComposerDraft({ mode: "compose", userId: 1 })?.text).toBe("書きかけ");
  });

  it("別ユーザーIDの下書きは分離される", () => {
    saveComposerDraft({ mode: "compose", userId: 1 }, makeDraft("alice分"));
    expect(loadComposerDraft({ mode: "compose", userId: 2 })).toBeNull();
  });

  it("本文・添付とも空なら保存されず、既存の下書きも消える", () => {
    saveComposerDraft({ mode: "compose", userId: 1 }, makeDraft("書きかけ"));
    saveComposerDraft({ mode: "compose", userId: 1 }, makeDraft(""));
    expect(loadComposerDraft({ mode: "compose", userId: 1 })).toBeNull();
  });

  it("clearComposerDraftで明示的に消せる", () => {
    saveComposerDraft({ mode: "compose", userId: 1 }, makeDraft("書きかけ"));
    clearComposerDraft({ mode: "compose", userId: 1 });
    expect(loadComposerDraft({ mode: "compose", userId: 1 })).toBeNull();
  });
});

describe("composerDraft: reply/quote", () => {
  it("(userId, postId)単位で分離される", () => {
    saveComposerDraft({ mode: "reply", userId: 1, postId: "post-a" }, makeDraft("返信A"));
    saveComposerDraft({ mode: "reply", userId: 1, postId: "post-b" }, makeDraft("返信B"));
    expect(loadComposerDraft({ mode: "reply", userId: 1, postId: "post-a" })?.text).toBe("返信A");
    expect(loadComposerDraft({ mode: "reply", userId: 1, postId: "post-b" })?.text).toBe("返信B");
  });

  it("replyとquoteは同じpostIdでも別の下書きとして扱う", () => {
    saveComposerDraft({ mode: "reply", userId: 1, postId: "post-a" }, makeDraft("返信"));
    saveComposerDraft({ mode: "quote", userId: 1, postId: "post-a" }, makeDraft("引用"));
    expect(loadComposerDraft({ mode: "reply", userId: 1, postId: "post-a" })?.text).toBe("返信");
    expect(loadComposerDraft({ mode: "quote", userId: 1, postId: "post-a" })?.text).toBe("引用");
  });

  it("最大10件を超えると最も古いものから消える", () => {
    for (let i = 0; i < 11; i++) {
      saveComposerDraft({ mode: "reply", userId: 1, postId: `post-${i}` }, makeDraft(`text-${i}`));
    }
    expect(loadComposerDraft({ mode: "reply", userId: 1, postId: "post-0" })).toBeNull();
    for (let i = 1; i < 11; i++) {
      expect(loadComposerDraft({ mode: "reply", userId: 1, postId: `post-${i}` })?.text).toBe(`text-${i}`);
    }
  });

  it("既存postIdへの再保存は最新扱いになり、削除順を後ろにずらす", () => {
    for (let i = 0; i < 10; i++) {
      saveComposerDraft({ mode: "reply", userId: 1, postId: `post-${i}` }, makeDraft(`text-${i}`));
    }
    // post-0 を触り直して最新扱いにする
    saveComposerDraft({ mode: "reply", userId: 1, postId: "post-0" }, makeDraft("text-0-updated"));
    // 11件目を追加すると、次に古い post-1 が消えるはず（post-0 は温存される）
    saveComposerDraft({ mode: "reply", userId: 1, postId: "post-10" }, makeDraft("text-10"));
    expect(loadComposerDraft({ mode: "reply", userId: 1, postId: "post-1" })).toBeNull();
    expect(loadComposerDraft({ mode: "reply", userId: 1, postId: "post-0" })?.text).toBe("text-0-updated");
  });
});

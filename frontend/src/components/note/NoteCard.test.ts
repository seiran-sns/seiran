import { describe, expect, it } from "vitest";
import type { ReactionSummary } from "../../api/client";
import type { ReactionUpdate } from "../../contexts/StreamingContext";
import { applyReactionUpdate, optimisticSetReaction } from "./NoteCard";

describe("optimisticSetReaction", () => {
  it("未リアクション状態から新規リアクションを追加する", () => {
    const result = optimisticSetReaction([], "🎉", true);
    expect(result).toEqual([{ emoji: "🎉", count: 1, reactedByMe: true }]);
  });

  it("既存の他人のリアクションに自分の分を加算する", () => {
    const reactions: ReactionSummary[] = [{ emoji: "🎉", count: 2, reactedByMe: false }];
    const result = optimisticSetReaction(reactions, "🎉", true);
    expect(result).toEqual([{ emoji: "🎉", count: 3, reactedByMe: true }]);
  });

  it("同じ絵文字を再指定すると取消（トグルオフ）になる", () => {
    const reactions: ReactionSummary[] = [{ emoji: "🎉", count: 1, reactedByMe: true }];
    const result = optimisticSetReaction(reactions, "🎉", false);
    expect(result).toEqual([]);
  });

  it("別の絵文字へ切り替えると旧リアクションが外れ新リアクションが付く", () => {
    const reactions: ReactionSummary[] = [{ emoji: "🎉", count: 1, reactedByMe: true }];
    const result = optimisticSetReaction(reactions, "👍", true);
    expect(result).toEqual([{ emoji: "👍", count: 1, reactedByMe: true }]);
  });

  it("count=0になった絵文字は配列から除去される", () => {
    const reactions: ReactionSummary[] = [
      { emoji: "🎉", count: 1, reactedByMe: true },
      { emoji: "👍", count: 3, reactedByMe: false },
    ];
    const result = optimisticSetReaction(reactions, "🎉", false);
    expect(result).toEqual([{ emoji: "👍", count: 3, reactedByMe: false }]);
  });
});

describe("applyReactionUpdate", () => {
  const baseUpdate: ReactionUpdate = {
    postId: "1",
    reactions: [{ emoji: "🎉", count: 2 }],
    reactorActorId: 99,
    reactorEmoji: "🎉",
  };

  it("自分自身の操作の場合はreactorEmojiと一致する絵文字のreactedByMeをtrueにする", () => {
    const result = applyReactionUpdate([], baseUpdate, 99);
    expect(result).toEqual([{ emoji: "🎉", count: 2, emojiUrl: undefined, reactedByMe: true }]);
  });

  it("自分自身の取消操作（reactorEmoji=null）ではどの絵文字もreactedByMeにならない", () => {
    const update: ReactionUpdate = { ...baseUpdate, reactorEmoji: null };
    const result = applyReactionUpdate([], update, 99);
    expect(result[0].reactedByMe).toBe(false);
  });

  it("他人の操作の場合は既知のreactedByMeをそのまま引き継ぐ", () => {
    const existing: ReactionSummary[] = [{ emoji: "🎉", count: 1, reactedByMe: true }];
    const result = applyReactionUpdate(existing, baseUpdate, 1 /* 自分のactor_idはreactorと異なる */);
    expect(result).toEqual([{ emoji: "🎉", count: 2, emojiUrl: undefined, reactedByMe: true }]);
  });

  it("myActorIdがundefined（未認証）の場合は常に他人の操作として扱う", () => {
    const result = applyReactionUpdate([], baseUpdate, undefined);
    expect(result[0].reactedByMe).toBe(false);
  });
});

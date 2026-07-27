import { describe, expect, it, vi } from "vitest";
import { getPollState, setPollState, subscribePollState } from "./pollVoteStore";

describe("pollVoteStore", () => {
  it("同じ投稿IDの購読者すべてへ投票結果を同期する", () => {
    const first = vi.fn();
    const second = vi.fn();
    const unsubscribeFirst = subscribePollState("poll-sync-test", first);
    const unsubscribeSecond = subscribePollState("poll-sync-test", second);
    const poll = {
      multiple: false,
      options: [
        { name: "A", votes: 1 },
        { name: "B", votes: 0 },
      ],
    };

    setPollState("poll-sync-test", { poll, votedByMe: [0] });

    expect(first).toHaveBeenCalledOnce();
    expect(second).toHaveBeenCalledOnce();
    expect(getPollState("poll-sync-test")).toEqual({ poll, votedByMe: [0] });
    unsubscribeFirst();
    unsubscribeSecond();
  });
});

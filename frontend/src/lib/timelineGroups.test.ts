import { describe, expect, it } from "vitest";
import type { Note } from "../api/client";
import { groupTimeline } from "./timelineGroups";

function post(id: string): Note {
  return { id } as Note;
}

function repost(id: string, target: string): Note {
  return { id, renoteId: target, renote: post(target) } as Note;
}

function ids(notes: Note[], breakBeforeIds?: Set<string>): string[][] {
  return groupTimeline(notes, breakBeforeIds).map((g) => g.notes.map((n) => n.id));
}

describe("groupTimeline", () => {
  it("同じポストのリポストの連続をまとめる", () => {
    const notes = [post("a"), repost("r1", "P"), repost("r2", "P"), repost("r3", "Q")];
    expect(ids(notes)).toEqual([["a"], ["r1", "r2"], ["r3"]]);
  });

  it("隣に並んだリポスト元のポスト自体も含める", () => {
    const notes = [repost("r1", "P"), post("P"), post("b")];
    expect(ids(notes)).toEqual([["r1", "P"], ["b"]]);
  });

  it("元投稿が見えないリポストも renoteId でまとめる", () => {
    const hidden = { id: "r2", renoteId: "P" } as Note;
    expect(ids([repost("r1", "P"), hidden])).toEqual([["r1", "r2"]]);
  });

  it("間に別のポストが挟まればまとめない", () => {
    const notes = [repost("r1", "P"), post("x"), repost("r2", "P")];
    expect(ids(notes)).toEqual([["r1"], ["x"], ["r2"]]);
  });

  it("取りこぼし区間の区切りをまたいでまとめない", () => {
    const notes = [repost("r1", "P"), repost("r2", "P")];
    expect(ids(notes, new Set(["r2"]))).toEqual([["r1"], ["r2"]]);
  });
});

import { act } from "react";
import { createRoot, Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { useCursorPagination } from "./useCursorPagination";

type Result = ReturnType<typeof useCursorPagination<string>>;

function deferred() {
  let resolve!: (rows: string[]) => void;
  const promise = new Promise<string[]>((r) => (resolve = r));
  return { promise, resolve };
}

let root: Root | null = null;

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  act(() => root?.unmount());
  root = null;
});

function render(fetchPage: (untilId: string) => Promise<string[]>, resetKey: string) {
  const state: { current: Result | null } = { current: null };
  function Probe({ k }: { k: string }) {
    state.current = useCursorPagination<string>(fetchPage, (s) => s, 2, () => {}, { items: ["a1", "a2"], hasMore: true }, k);
    return null;
  }
  root = createRoot(document.createElement("div"));
  act(() => root!.render(<Probe k={resetKey} />));
  return {
    state,
    rerender: (k: string) => act(() => root!.render(<Probe k={k} />)),
  };
}

describe("useCursorPagination resetKey", () => {
  it("resetKeyが変わる前に始めた次ページ取得は、完了しても一覧へ追記しない", async () => {
    const pending = deferred();
    const { state, rerender } = render(() => pending.promise, "local");

    act(() => state.current!.loadMore());
    expect(state.current!.loadingMore).toBe(true);

    rerender("global");
    act(() => state.current!.setItems(["g1", "g2"]));
    expect(state.current!.loadingMore).toBe(false);

    await act(async () => pending.resolve(["a3", "a4"]));
    expect(state.current!.items).toEqual(["g1", "g2"]);
    expect(state.current!.hasMore).toBe(true);
  });

  it("古い世代の取得が残っていても、新しい世代の次ページ取得は始められる", async () => {
    const first = deferred();
    const second = deferred();
    const calls: string[] = [];
    const queue = [first, second];
    const { state, rerender } = render((untilId) => {
      calls.push(untilId);
      return queue.shift()!.promise;
    }, "local");

    act(() => state.current!.loadMore());
    rerender("global");
    act(() => state.current!.setItems(["g1", "g2"]));
    act(() => state.current!.loadMore());
    expect(calls).toEqual(["a2", "g2"]);

    await act(async () => first.resolve(["a3"]));
    expect(state.current!.loadingMore).toBe(true);
    await act(async () => second.resolve(["g3"]));
    expect(state.current!.items).toEqual(["g1", "g2", "g3"]);
    expect(state.current!.loadingMore).toBe(false);
  });
});

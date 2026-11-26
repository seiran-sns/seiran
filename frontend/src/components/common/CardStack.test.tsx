import { act } from "react";
import { createRoot, Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import CardStack from "./CardStack";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// 広げる・畳むアニメーションは prefers-reduced-motion なら即座に切り替わる。テストではそれで確かめる。
beforeEach(() => {
  vi.stubGlobal(
    "matchMedia",
    (query: string) =>
      ({
        matches: query.includes("prefers-reduced-motion"),
        media: query,
        addEventListener: () => {},
        removeEventListener: () => {},
      }) as unknown as MediaQueryList,
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const KEYS = ["a", "b", "c"];

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

function render(props: Partial<Parameters<typeof CardStack>[0]> = {}) {
  act(() =>
    root.render(
      <div>
        <div data-testid="outside">outside</div>
        <CardStack
          keys={KEYS}
          renderCard={(i) => <button type="button">{KEYS[i]}</button>}
          className="stack"
          spreadOffsetPx={32}
          pinnedClassName="pinned"
          {...props}
        />
      </div>,
    ),
  );
}

const stack = () => container.querySelector<HTMLElement>(".stack")!;
const card = (key: string) => [...stack().children].find((c) => c.textContent === key) as HTMLElement;
const tops = () => KEYS.map((k) => card(k).style.top);

// React は mouseenter / mouseleave を mouseover / mouseout から作る。
function moveMouse(from: Element, to: Element) {
  act(() => {
    from.dispatchEvent(new MouseEvent("mouseout", { bubbles: true, relatedTarget: to }));
    to.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, relatedTarget: from }));
  });
}

describe("CardStack の広げる・固定する", () => {
  it("マウスが乗っている間だけ spreadOffsetPx ずつずらす", () => {
    render();
    const outside = container.querySelector("[data-testid=outside]")!;
    expect(tops()).toEqual(["0px", "8px", "16px"]);
    moveMouse(outside, card("b"));
    expect(tops()).toEqual(["0px", "32px", "64px"]);
    moveMouse(card("b"), outside);
    expect(tops()).toEqual(["0px", "8px", "16px"]);
  });

  it("広げたまま押すと固定し、枠を付けて広げたままにする。もう一度乗せると固定が解ける", () => {
    const onSpreadChange = vi.fn();
    render({ onSpreadChange });
    const outside = container.querySelector("[data-testid=outside]")!;
    moveMouse(outside, card("b"));
    act(() => card("b").querySelector("button")!.click());
    expect(onSpreadChange).toHaveBeenLastCalledWith(true);
    // 乗せている間は枠を出さない（乗せれば消えるものとして見せる）。
    expect(stack().classList.contains("pinned")).toBe(false);
    moveMouse(card("b"), outside);
    expect(stack().classList.contains("pinned")).toBe(true);
    expect(tops()).toEqual(["0px", "32px", "64px"]);
    moveMouse(outside, card("c"));
    expect(onSpreadChange).toHaveBeenLastCalledWith(false);
    expect(stack().classList.contains("pinned")).toBe(false);
    moveMouse(card("c"), outside);
    expect(tops()).toEqual(["0px", "8px", "16px"]);
  });

  it("戻ってきたときは覚えていた固定と前面のカードで出す", () => {
    render({ initialSpread: true, initialFront: "b" });
    expect(stack().classList.contains("pinned")).toBe(true);
    expect(tops()).toEqual(["0px", "32px", "64px"]);
    // 前面のカードは最後に描く。
    expect(stack().lastElementChild?.textContent).toBe("b");
  });

  it("spreadOffsetPx が無ければ広げない（タイムライン）", () => {
    render({ spreadOffsetPx: undefined });
    const outside = container.querySelector("[data-testid=outside]")!;
    moveMouse(outside, card("b"));
    expect(tops()).toEqual(["0px", "8px", "16px"]);
    act(() => card("b").querySelector("button")!.click());
    moveMouse(card("b"), outside);
    expect(stack().classList.contains("pinned")).toBe(false);
  });
});

describe("CardStack（タッチ環境）", () => {
  beforeEach(() => {
    vi.stubGlobal(
      "matchMedia",
      (query: string) =>
        ({
          matches: query.includes("hover: none") || query.includes("prefers-reduced-motion"),
          media: query,
          addEventListener: () => {},
          removeEventListener: () => {},
        }) as unknown as MediaQueryList,
    );
  });

  it("広げない。タップはそのままカードへ届く", () => {
    const onCardClick = vi.fn();
    render({ renderCard: (i) => <button type="button" onClick={onCardClick}>{KEYS[i]}</button> });
    act(() => card("b").querySelector("button")!.click());
    expect(onCardClick).toHaveBeenCalledTimes(1);
    expect(tops()).toEqual(["0px", "8px", "16px"]);
  });

  it("PC で固定したまとまりの記憶があっても広げない", () => {
    render({ initialSpread: true });
    expect(tops()).toEqual(["0px", "8px", "16px"]);
    expect(stack().classList.contains("pinned")).toBe(false);
  });
});

import { act } from "react";
import { createRoot, Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import CardStack, { collapsedStackIndexAt } from "./CardStack";

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

describe("CardStack の広げる（タッチ環境）", () => {
  // 画面の上から 25% の線（jsdom の innerHeight 768 なら 192px）がまとまりにかかっているかで決める。
  function touchEnv(stackTop: number, stackBottom: number) {
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
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const top = this.classList.contains("stack") ? stackTop : 0;
      const bottom = this.classList.contains("stack") ? stackBottom : 0;
      return { top, bottom, height: bottom - top, left: 0, right: 0, width: 0, x: 0, y: top } as DOMRect;
    });
  }

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("線がかかっているまとまりは広げる", () => {
    touchEnv(100, 300);
    const onSpreadChange = vi.fn();
    render({ onSpreadChange });
    expect(tops()).toEqual(["0px", "32px", "64px"]);
    expect(onSpreadChange).toHaveBeenLastCalledWith(true);
    // タッチ環境では固定の枠は出さない。
    expect(stack().classList.contains("pinned")).toBe(false);
  });

  it("線がかかっていないまとまりは広げない", () => {
    touchEnv(400, 600);
    render();
    expect(tops()).toEqual(["0px", "8px", "16px"]);
  });

  it("広げた形で戻ってきたときは、スクロール位置が戻る前に線がかかっていなくても畳まない", () => {
    touchEnv(400, 600);
    render({ initialSpread: true });
    expect(tops()).toEqual(["0px", "32px", "64px"]);
  });
});

describe("collapsedStackIndexAt", () => {
  // カード1枚の高さ 60px、畳んだずらし幅 8px。
  const height = (count: number) => 60 + (count - 1) * 8;

  it("少ない件数では中ほどのカードにかからず、先頭と末尾で半分ずつ", () => {
    const h = height(3); // 76px、先頭・末尾に 38px ずつ
    expect(collapsedStackIndexAt(0, h, 60, 3)).toBe(0);
    expect(collapsedStackIndexAt(37, h, 60, 3)).toBe(0);
    expect(collapsedStackIndexAt(38, h, 60, 3)).toBe(2);
    expect(collapsedStackIndexAt(75, h, 60, 3)).toBe(2);
  });

  it("多い件数では先頭と末尾はカード1枚の高さまでで、残りを中ほどのカードで等分する", () => {
    const h = height(12); // 148px、先頭・末尾に 60px ずつ、中ほどの10枚で 28px を等分
    expect(collapsedStackIndexAt(59, h, 60, 12)).toBe(0);
    expect(collapsedStackIndexAt(60, h, 60, 12)).toBe(1);
    expect(collapsedStackIndexAt(60 + 2.8 * 5, h, 60, 12)).toBe(6);
    expect(collapsedStackIndexAt(87.9, h, 60, 12)).toBe(10);
    expect(collapsedStackIndexAt(88, h, 60, 12)).toBe(11);
  });

  it("2件なら半分ずつ", () => {
    expect(collapsedStackIndexAt(33, height(2), 60, 2)).toBe(0);
    expect(collapsedStackIndexAt(34, height(2), 60, 2)).toBe(1);
  });
});

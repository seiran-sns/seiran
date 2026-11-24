import { expect, test } from "@playwright/test";
import { registerUserViaApi, seedAuth } from "../fixtures/api-helpers";

test("モバイルでも投稿エディタの文字サイズを16px以上に保つ", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2ecomposerzoom");
  await page.setViewportSize({ width: 390, height: 844 });
  await seedAuth(page, author.token);
  await page.goto("/");

  const editor = page.locator('[contenteditable="true"]').first();
  await expect(editor).toBeVisible();
  await expect(editor).toHaveCSS("font-size", "16px");
});

test("投稿本文のメンション候補をキーボードで選択し既知IDとして表示できる", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ecomposer");
  const target = await registerUserViaApi(request, "e2ecomposertarget");
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  await editor.fill(`@${target.username.slice(0, -2)}`);

  const option = page.getByRole("option").filter({ hasText: `@${target.username}` });
  await expect(option).toBeVisible();
  await editor.press("Enter");

  await expect(editor).toContainText(`@${target.username}`);
  const mention = editor.locator("span", { hasText: `@${target.username}` });
  await expect(mention).toHaveCSS("font-weight", "700");
  await expect(mention).toHaveCSS("color", "rgb(22, 139, 210)");
});

test("ローカルユーザーは短いIDで候補表示し、3形式のIDを既知として扱う", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ecomposerlocal");
  const target = await registerUserViaApi(request, "e2ecomposerlocaltarget");
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  await editor.fill(`@${target.username.slice(0, -2)}`);
  const option = page.getByRole("option").filter({ hasText: target.username });
  await expect(option).toBeVisible();
  await expect(option.locator("small")).toHaveText(`@${target.username}`);

  const search = await request.get(`/api/actors/search?q=${target.username}`, {
    headers: { Authorization: `Bearer ${author.token}` },
  });
  const actors = (await search.json()) as { username: string; domain: string }[];
  const local = actors.find((actor) => actor.username === target.username)!;
  const bskySuggest = await request.get(
    `/api/actors/suggest?q=${encodeURIComponent(`${target.username}.${local.domain}`)}`,
    { headers: { Authorization: `Bearer ${author.token}` } }
  );
  expect(bskySuggest.ok()).toBeTruthy();
  const bskyRows = (await bskySuggest.json()) as { actor_id: string; target: string }[];
  expect(bskyRows.find((actor) => actor.actor_id === target.actorId)?.target).toBe(
    `${target.username.toLowerCase()}.${local.domain}`
  );

  const substringSuggest = await request.get(
    `/api/actors/suggest?q=${encodeURIComponent(target.username.slice(2))}`,
    { headers: { Authorization: `Bearer ${author.token}` } }
  );
  const substringRows = (await substringSuggest.json()) as { actor_id: string }[];
  expect(substringRows.some((actor) => actor.actor_id === target.actorId)).toBe(false);

  for (const mention of [
    `@${target.username}`,
    `@${target.username}.${local.domain}`,
    `@${target.username}@${local.domain}`,
  ]) {
    await editor.fill(mention);
    await expect(editor.locator("span", { hasText: mention })).toHaveCSS("color", "rgb(22, 139, 210)");
  }
});

test("逐次入力中にID装飾が更新されてもFedi/Bsky候補と完全修飾ID認識を維持する", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ecomposerremote");
  await page.route("**/api/actors/suggest?**", (route) =>
    route.fulfill({
      json: [
        {
          actor_id: "9001",
          username: "fedi-user",
          domain: "remote.example",
          display_name: "Fedi user",
          actor_type: "fedi",
          avatar_url: null,
          target: "fedi-user@remote.example",
        },
        {
          actor_id: "9002",
          username: "sky-user.bsky.social",
          domain: "",
          display_name: "Bsky user",
          actor_type: "bsky",
          avatar_url: null,
          target: "sky-user.bsky.social",
        },
      ],
    })
  );
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  await editor.click();
  // 部分IDの未知/既知チェック結果が入力途中に返り、装飾DOMが更新される状況を再現する。
  await editor.pressSequentially("@fedi-user@remote.example", { delay: 80 });
  await expect(page.getByRole("option").filter({ hasText: "@fedi-user@remote.example" })).toBeVisible();
  await expect(editor.locator("span", { hasText: "@fedi-user@remote.example" })).toHaveCSS(
    "color",
    "rgb(22, 139, 210)"
  );

  await editor.fill("");
  await editor.pressSequentially("@sky-user.bsky.social", { delay: 80 });
  await expect(page.getByRole("option").filter({ hasText: "@sky-user.bsky.social" })).toBeVisible();
  await expect(editor.locator("span", { hasText: "@sky-user.bsky.social" })).toHaveCSS(
    "color",
    "rgb(22, 139, 210)"
  );
});

test("カスタム絵文字候補を画像へ置換し直後のBackspaceで丸ごと消せる", async ({ page, request }) => {
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  const author = await registerUserViaApi(request, "e2ecomposeremoji");
  await page.route("**/api/emojis", (route) =>
    route.fulfill({
      json: {
        emojis: [{
          id: "1",
          aliases: ["long"],
          name: "wide_emoji",
          category: null,
          host: null,
          url: "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='80' height='20'/%3E",
          license: null,
        }],
      },
    })
  );
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  // Bsky配送オン中はカスタム絵文字shortcodeを画像展開せず警告色のテキストのまま表示する
  // 仕様のため、画像化そのものを検証するこのテストではBsky配送をオフにする。
  await page.getByRole("button", { name: "Blueskyに配送" }).click();

  const editor = page.locator('[contenteditable="true"]').first();
  await expect(editor).toBeVisible();
  await editor.click();
  await editor.pressSequentially(":wide");
  await expect(page.getByRole("option", { name: /:wide_emoji:/ })).toBeVisible();
  await editor.press("ArrowDown");
  await editor.press("Enter");

  const emoji = editor.getByRole("img", { name: ":wide_emoji:" });
  await expect(emoji).toBeVisible();
  // 絵文字画像の高さはCSS側で`1.6em`（行の高さいっぱい）指定のため、フォントサイズに
  // 追従する相対値になる。ブラウザのサブピクセル丸めを吸収するため近似値で比較する。
  const emojiHeight = await emoji.evaluate((el) => parseFloat(getComputedStyle(el).height));
  expect(emojiHeight).toBeGreaterThan(20);
  expect(emojiHeight).toBeLessThan(30);
  await expect(page.getByRole("listbox", { name: "入力候補" })).toHaveCount(0);

  await editor.press("Backspace");
  await page.waitForTimeout(100);
  expect(pageErrors).toEqual([]);
  await expect(emoji).toHaveCount(0);
  await expect(editor).not.toContainText("wide_emoji");
});

test("絵文字の直前のBackspaceは手前の文字を消し、行頭が絵文字の行でHome→Backspaceすると前の行と結合する", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2ecomposeremojicaret");
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  await editor.click();
  await page.keyboard.insertText("ab 😀 cd");
  await expect(editor.getByRole("img", { name: "😀" })).toHaveCount(1);
  // "ab |😀 cd" へ戻してBackspace。絵文字ではなく手前の空白が消える。
  for (let i = 0; i < 4; i += 1) await editor.press("ArrowLeft");
  await editor.press("Backspace");
  await editor.press("End");
  await editor.press("Enter");
  await page.keyboard.insertText("😀");
  await editor.pressSequentially("y");
  // 行頭の絵文字の手前へ移動してBackspace。改行が消えて前の行と結合する。
  await editor.press("Home");
  await editor.press("Backspace");

  const [response] = await Promise.all([
    page.waitForResponse(
      (res) => res.url().includes("/api/notes/create") && res.request().method() === "POST"
    ),
    editor.press("Control+Enter"),
  ]);
  const note = (await response.json()) as { text: string };
  expect(note.text).toBe("ab😀 cd😀y");
});

test("連結したshortcodeは閉じコロン後が英数字でない最後のものだけ絵文字化する", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ecomposeremojiboundary");
  await page.route("**/api/emojis", (route) =>
    route.fulfill({
      json: {
        emojis: [{
          id: "1",
          aliases: [],
          name: "igyo",
          category: null,
          host: null,
          url: "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==",
          license: null,
        }],
      },
    })
  );
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  // Bsky配送オン中はカスタム絵文字shortcodeを画像展開せず警告色のテキストのまま表示する
  // 仕様のため、画像化そのものを検証するこのテストではBsky配送をオフにする。
  await page.getByRole("button", { name: "Blueskyに配送" }).click();

  const editor = page.locator('[contenteditable="true"]').first();
  await editor.fill(":igyo:igyo:igyo:");
  await expect(editor.getByRole("img", { name: ":igyo:" })).toHaveCount(1);
  // 画像化した最後のshortcodeはtextContentに含まれないため、手前の2つが
  // 通常テキストのまま残ることを確認する。
  await expect(editor).toContainText(":igyo:igyo");
});

test("Bsky配送オン中はカスタム絵文字shortcodeを警告色のまま表示し、配送ボタンに警告が出る", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2ecomposerbskywarn");
  await page.route("**/api/emojis", (route) =>
    route.fulfill({
      json: {
        emojis: [{
          id: "1",
          aliases: [],
          name: "warn_emoji",
          category: null,
          host: null,
          url: "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='20' height='20'/%3E",
          license: null,
        }],
      },
    })
  );
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  // 新規投稿フォームはデフォルトでBsky配送オン。
  const bskyBtn = page.getByRole("button", { name: "Blueskyに配送" });
  const editor = page.locator('[contenteditable="true"]').first();
  await editor.fill(":warn_emoji:");

  // Bsky配送オン中は画像化されず、警告色のテキストのまま残る。
  await expect(editor.getByRole("img", { name: ":warn_emoji:" })).toHaveCount(0);
  await expect(editor).toContainText(":warn_emoji:");
  await expect(bskyBtn).toHaveAttribute(
    "title",
    "Blueskyに配送\n⚠️Bluesky向けには本文内カスタム絵文字が埋め込めません"
  );

  // Bsky配送をオフにすると画像展開に戻り、警告も消える。
  await bskyBtn.click();
  await expect(editor.getByRole("img", { name: ":warn_emoji:" })).toBeVisible();
  await expect(bskyBtn).toHaveAttribute("title", "Blueskyに配送");
});

test("入力候補は書き足したときだけ開き、決定・Escで閉じ、既存IDへカーソルが入っただけでは開かない", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2ecomposersuggestopen");
  await page.route("**/api/actors/suggest?**", (route) =>
    route.fulfill({
      json: [{
        actor_id: "9001",
        username: "fedi-user",
        domain: "remote.example",
        display_name: "Fedi user",
        actor_type: "fedi",
        avatar_url: null,
        target: "fedi-user@remote.example",
      }],
    })
  );
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  const listbox = page.getByRole("listbox", { name: "入力候補" });
  await editor.click();
  await editor.pressSequentially("@fedi");
  await expect(listbox).toBeVisible();
  await editor.press("Enter");
  await expect(editor).toHaveText("@fedi-user@remote.example");
  await expect(listbox).toHaveCount(0);

  await editor.pressSequentially(" ok");
  // 既存IDの末尾・内側へカーソルを動かしても候補は開かない。
  for (let i = 0; i < 6; i += 1) await editor.press("ArrowLeft");
  await page.waitForTimeout(500);
  await expect(listbox).toHaveCount(0);

  // IDに文字を書き足すと開き、Escで閉じる。同じIDの中を動いても閉じたまま。
  await editor.pressSequentially("x");
  await expect(listbox).toBeVisible();
  await editor.press("Escape");
  await expect(listbox).toHaveCount(0);
  await editor.press("ArrowLeft");
  await page.waitForTimeout(500);
  await expect(listbox).toHaveCount(0);
});

test("上下矢印で入力候補の選択を移動できる", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ecomposerarrows");
  await seedAuth(page, author.token);
  await page.route("**/api/emojis", (route) =>
    route.fulfill({
      json: {
        emojis: [
          { id: "1", aliases: [], name: "first", category: null, host: null, url: "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==" },
          { id: "2", aliases: [], name: "second", category: null, host: null, url: "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==" },
        ],
      },
    })
  );
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  await editor.fill(":");
  const options = page.getByRole("option");
  await expect(options).toHaveCount(2);
  await expect(options.nth(0)).toHaveAttribute("aria-selected", "true");
  await editor.press("ArrowDown");
  await expect(options.nth(1)).toHaveAttribute("aria-selected", "true");
  await editor.press("ArrowUp");
  await expect(options.nth(0)).toHaveAttribute("aria-selected", "true");
});

test("IME未確定中は入力DOMへ干渉せず、確定後に本文へ同期する", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ecomposerime");
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  await editor.evaluate((element) => {
    element.dispatchEvent(new CompositionEvent("compositionstart", {
      bubbles: true,
      data: "",
    }));
    const composingText = document.createTextNode("にほん");
    element.replaceChildren(composingText);
    Object.assign(window, { __composerComposingText: composingText });
    element.dispatchEvent(new InputEvent("input", {
      bubbles: true,
      data: "にほん",
      inputType: "insertCompositionText",
      isComposing: true,
    }));
  });

  await expect(editor).toHaveText("にほん");
  await expect.poll(() => editor.evaluate(
    (element) => element.firstChild === (window as typeof window & {
      __composerComposingText?: Node;
    }).__composerComposingText
  )).toBe(true);

  await editor.evaluate((element) => {
    element.textContent = "日本";
    element.dispatchEvent(new CompositionEvent("compositionend", {
      bubbles: true,
      data: "日本",
    }));
  });
  await expect(editor).toHaveText("日本");
});

test("IME変換中にメンション存在確認が返っても未確定文字を消さず、確定後に装飾を反映する", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2ecomposerimemention");
  const target = await registerUserViaApi(request, "e2ecomposerimetarget");
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  // 存在確認の応答を変換開始後まで止めておき、変換中に結果が届く状況を作る。
  let release!: () => void;
  const gate = new Promise<void>((resolve) => (release = resolve));
  await page.route("**/api/actors/suggest**", async (route) => {
    await gate;
    await route.continue();
  });

  const editor = page.locator('[contenteditable="true"]').first();
  await editor.fill(`@${target.username} `);
  await editor.evaluate((element) => {
    element.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
    const composingText = document.createTextNode("にほん");
    element.append(composingText);
    Object.assign(window, { __composerComposingText: composingText });
    element.dispatchEvent(new InputEvent("input", {
      bubbles: true,
      data: "にほん",
      inputType: "insertCompositionText",
      isComposing: true,
    }));
  });

  const suggested = page.waitForResponse((res) => res.url().includes("/api/actors/suggest"));
  release();
  await suggested;
  await page.waitForTimeout(500);
  expect(await editor.evaluate(() => (window as typeof window & {
    __composerComposingText?: Node;
  }).__composerComposingText?.isConnected)).toBe(true);

  await editor.evaluate(() => {
    const node = (window as typeof window & { __composerComposingText?: Text }).__composerComposingText!;
    node.data = "日本";
    node.parentElement!.closest('[contenteditable="true"]')!.dispatchEvent(
      new CompositionEvent("compositionend", { bubbles: true, data: "日本" }),
    );
  });
  await expect(editor).toHaveText(`@${target.username} 日本`);
  await expect(editor.locator("span", { hasText: `@${target.username}` })).toHaveCSS(
    "color",
    "rgb(22, 139, 210)",
  );
});

test("メンションの直後で始めたIME変換の未確定文字はメンションの装飾を受けず、確定後の本文に余計な文字を残さない", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2ecomposerimeafter");
  const target = await registerUserViaApi(request, "e2ecomposerimeaftertarget");
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  await editor.click();
  await editor.pressSequentially(`@${target.username}`);
  const mention = editor.locator("span", { hasText: `@${target.username}` });
  await expect(mention).toHaveCSS("color", "rgb(22, 139, 210)");

  // 実際のIMEと同じ経路で未確定文字を入れるため、CDPで変換を起こす。
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Input.imeSetComposition", { text: "てす", selectionStart: 2, selectionEnd: 2 });
  await expect(mention).toHaveText(`@${target.username}`);
  await cdp.send("Input.insertText", { text: "テスト" });
  await expect(editor).toHaveText(`@${target.username}テスト`);

  const [response] = await Promise.all([
    page.waitForResponse(
      (res) => res.url().includes("/api/notes/create") && res.request().method() === "POST"
    ),
    editor.press("Control+Enter"),
  ]);
  const note = (await response.json()) as { text: string };
  expect(note.text).toBe(`@${target.username}テスト`);
});

test("Enterキー1回につき改行が1個だけ入り揺り戻しが起きない", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ecomposerenter");
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  await editor.click();
  await editor.pressSequentially("あ");
  await editor.press("Enter");
  await editor.pressSequentially("い");
  await editor.press("Enter");
  await editor.press("Enter");
  await editor.pressSequentially("う");

  const [response] = await Promise.all([
    page.waitForResponse(
      (res) => res.url().includes("/api/notes/create") && res.request().method() === "POST"
    ),
    editor.press("Control+Enter"),
  ]);
  const note = (await response.json()) as { text: string };
  expect(note.text).toBe("あ\nい\n\nう");
});

test("IME未確定文字が入力されるとプレースホルダーが消える", async ({ page, request }) => {
  const author = await registerUserViaApi(request, "e2ecomposerplaceholder");
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  await expect(editor.evaluate((element) => element.matches(":empty"))).resolves.toBe(true);

  await editor.evaluate((element) => {
    element.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
    element.replaceChildren(document.createTextNode("にほ"));
    element.dispatchEvent(new InputEvent("input", {
      bubbles: true,
      data: "にほ",
      inputType: "insertCompositionText",
      isComposing: true,
    }));
  });

  await expect(editor.evaluate((element) => element.matches(":empty"))).resolves.toBe(false);
});

test("Safariの漢字変換確定Enter（compositionendがkeydownより先行しkeyCode 229）で改行が入らない", async ({
  page,
  request,
}) => {
  const author = await registerUserViaApi(request, "e2ecomposerimeenter");
  await seedAuth(page, author.token);
  await page.goto("/");
  await page.waitForTimeout(2_000);

  const editor = page.locator('[contenteditable="true"]').first();
  await editor.click();

  await editor.evaluate((element) => {
    element.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" }));
    element.replaceChildren(document.createTextNode("にほん"));
    element.dispatchEvent(new InputEvent("input", {
      bubbles: true,
      data: "にほん",
      inputType: "insertCompositionText",
      isComposing: true,
    }));
  });

  // SafariはIME確定のEnterで、確定によるcompositionendがEnterのkeydownより
  // 先に発火する。この時点でcomposing.currentは既にfalseへ戻っているため、
  // keyCode 229でしか変換確定のEnterだと判別できない。
  await editor.evaluate((element) => {
    element.textContent = "日本";
    element.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "日本" }));
    element.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        keyCode: 229,
        bubbles: true,
        cancelable: true,
      }),
    );
  });

  await expect(editor).toHaveText("日本");
});

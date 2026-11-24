import {
  ClipboardEvent,
  KeyboardEvent,
  forwardRef,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { ActorSuggestion, PublicEmoji, api } from "../../api/client";
import { fetchCustomEmojis } from "../../lib/customEmojis";
import { SHORTCODE_SOURCE, WORD_CHAR_RE } from "../../lib/richTextPatterns";
import { findTwemojiMatches } from "../../lib/twemoji";
import RemoteImg from "../common/RemoteImg";
import styles from "./ComposerEditor.module.css";
import DisplayName from "../common/DisplayName";

/** `PostComposer` から絵文字ショートコード・ユーザーIDピッカーで選んだ値をカーソル位置へ
 * 挿入するために、本文DOM要素へ`insertAtCursor`を生やして公開する。 */
export interface ComposerEditorHandle extends HTMLDivElement {
  insertAtCursor: (text: string) => void;
}

type Candidate =
  | { kind: "actor"; key: string; value: string; actor: ActorSuggestion }
  | { kind: "emoji"; key: string; value: string; emoji: PublicEmoji };

interface ComposerEditorProps {
  value: string;
  onChange: (value: string) => void;
  onSubmitShortcut: () => void;
  onImagePaste: (file: File) => void;
  placeholder: string;
  autoFocus?: boolean;
  /** Bsky配送オンの間はカスタム絵文字ショートコードを画像展開せず、警告色のテキストの
   * ままにする（Bsky側では本文内カスタム絵文字を埋め込めないため）。 */
  federateToBsky?: boolean;
}

const MENTION_RE =
  /(?<![\w])@[A-Za-z0-9_-]+(?:\.[A-Za-z0-9-]+)*(?:@[A-Za-z0-9.-]+)?/g;
const TOKEN_RE = /(?<![\w])@[A-Za-z0-9_.@-]*$|:[A-Za-z0-9_+-]*$/;
const DECORATION_RE = new RegExp(
  `(${SHORTCODE_SOURCE}(?![A-Za-z0-9_])|(?<![\\w])@[A-Za-z0-9_-]+(?:\\.[A-Za-z0-9-]+)*(?:@[A-Za-z0-9.-]+)?)`,
  "g",
);

/**
 * 絵文字・ユーザーIDピッカー（`PostComposer`）から本文へ`text`を挿入する際の、挿入後の
 * 本文全体とキャレット位置を計算する（DOM操作を含まない純粋関数、テスト用にexport）。
 * `caret`が既存のショートコード/メンションの内側にある場合は、その直後へ挿入する。
 * 挿入直後に半角英数字が続くとショートコード/メンションとして認識
 * されなくなる（`DECORATION_RE`の右端境界と同じ規則）ため、その場合は半角スペースを
 * 1つ追加で挿入する。
 */
export function resolveInsertion(
  value: string,
  caret: number,
  text: string,
): { next: string; caret: number } {
  const enclosing = Array.from(value.matchAll(DECORATION_RE)).find(
    (m) => m.index! < caret && caret < m.index! + m[0].length,
  );
  const start = enclosing ? enclosing.index! + enclosing[0].length : caret;
  const needsSpace = WORD_CHAR_RE.test(value.charAt(start));
  const inserted = needsSpace ? `${text} ` : text;
  return {
    next: `${value.slice(0, start)}${inserted}${value.slice(start)}`,
    caret: start + inserted.length,
  };
}

function nodeValue(node: Node): string {
  if (node instanceof HTMLElement && node.dataset.value)
    return node.dataset.value;
  if (node.nodeType === Node.TEXT_NODE) return node.textContent ?? "";
  if (node.nodeName === "BR") {
    // 末尾の改行を表示上のキャレット配置のために足した番人brは値に含めない。
    if (node instanceof HTMLElement && node.dataset.pad) return "";
    return "\n";
  }
  return Array.from(node.childNodes).map(nodeValue).join("");
}

function selectionOffset(root: HTMLElement): number | null {
  const selection = window.getSelection();
  if (!selection?.rangeCount || !root.contains(selection.anchorNode))
    return null;
  const range = selection.getRangeAt(0).cloneRange();
  range.selectNodeContents(root);
  range.setEnd(selection.anchorNode!, selection.anchorOffset);
  const holder = document.createElement("div");
  holder.append(range.cloneContents());
  return nodeValue(holder).length;
}

function restoreSelection(root: HTMLElement, target: number) {
  let consumed = 0;
  const range = document.createRange();
  const visit = (node: Node): boolean => {
    const value = nodeValue(node);
    if (node instanceof HTMLElement && node.dataset.value) {
      if (target <= consumed) range.setStartBefore(node);
      else if (target <= consumed + value.length) range.setStartAfter(node);
      else {
        consumed += value.length;
        return false;
      }
      return true;
    }
    if (node.nodeType === Node.TEXT_NODE) {
      if (target <= consumed + value.length) {
        range.setStart(node, Math.max(0, target - consumed));
        return true;
      }
      consumed += value.length;
      return false;
    }
    if (node.nodeName === "BR") {
      if (target <= consumed) {
        range.setStartBefore(node);
        return true;
      }
      consumed += 1;
      return false;
    }
    for (const child of Array.from(node.childNodes)) {
      if (visit(child)) return true;
    }
    return false;
  };
  if (!visit(root)) {
    range.selectNodeContents(root);
    range.collapse(false);
  } else {
    range.collapse(true);
  }
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

/** 装飾の更新で本文DOMを作り直す前に、フォーカス中ならキャレット位置を控えておく
 * （作り直した後のuseLayoutEffectで復元する）。 */
function preserveCaretForRender(
  editor: HTMLElement | null,
  pendingCaret: { current: number | null },
) {
  if (editor && document.activeElement === editor) {
    pendingCaret.current = selectionOffset(editor);
  }
}

function actorValue(actor: ActorSuggestion) {
  return `@${actor.target}`;
}

function actorMentionValues(actor: ActorSuggestion) {
  if (actor.actor_type !== "local") return [actorValue(actor)];
  return [
    `@${actor.username}`,
    `@${actor.username}.${actor.domain}`,
    `@${actor.username}@${actor.domain}`,
  ];
}

async function mentionIsKnown(mention: string) {
  const target = mention.slice(1);
  const queries = [target];
  if (!target.includes("@") && target.includes("."))
    queries.push(target.slice(0, target.indexOf(".")));
  const rows = (
    await Promise.all(queries.map((query) => api.actors.suggest(query, 10)))
  ).flat();
  return rows.some((row) =>
    actorMentionValues(row).some(
      (candidate) => candidate.toLowerCase() === mention.toLowerCase(),
    ),
  );
}

function escapeHtml(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** 絵文字1つ分のatomicノード。`nodeValue`/`restoreSelection`はdata-value属性を持つ要素を
 * そのまま1つの値として扱う。`<span contenteditable="false">`で包まず素の`<img>`にするのは、
 * 包むと直後にテキストノードが無い位置（行末の絵文字の直後など）でWebKitがキャレットを
 * 行の右端や左端に描き、Chromium/FirefoxではHomeでキャレットがspanの内側へ入って
 * 行頭からのBackspaceが効かなくなるため。`<img>`はブラウザが1文字分の置換要素として
 * 扱うので、キャレット移動・描画・削除をネイティブ動作に任せられる。 */
function emojiImgHtml(value: string, src: string, className: string): string {
  return `<img class="${className}" data-value="${escapeHtml(value)}" src="${escapeHtml(src)}" alt="${escapeHtml(value)}" title="${escapeHtml(value)}" draggable="false">`;
}

/** プレーンテキスト片中のUnicode絵文字を、カスタム絵文字ショートコードと同じatomicノード
 * （`emojiImgHtml`）としてtwemoji画像へ変換したHTMLにする。 */
function twemojiHtml(text: string, emojiClassName: string): string {
  const matches = findTwemojiMatches(text);
  if (matches.length === 0) return escapeHtml(text).replace(/\n/g, "<br>");
  let html = "";
  let cursor = 0;
  for (const m of matches) {
    const [start, end] = m.indices;
    if (start > cursor) {
      html += escapeHtml(text.slice(cursor, start)).replace(/\n/g, "<br>");
    }
    html += emojiImgHtml(m.text, m.url, emojiClassName);
    cursor = end;
  }
  if (cursor < text.length) {
    html += escapeHtml(text.slice(cursor)).replace(/\n/g, "<br>");
  }
  return html;
}

const ComposerEditor = forwardRef<ComposerEditorHandle, ComposerEditorProps>(function ComposerEditor(
  { value, onChange, onSubmitShortcut, onImagePaste, placeholder, autoFocus, federateToBsky },
  forwardedRef,
) {
  const { t } = useTranslation();
  const editorRef = useRef<HTMLDivElement>(null);
  const pendingCaret = useRef<number | null>(null);
  const composing = useRef(false);
  const initialCaret = useRef(value.length);
  const [caret, setCaret] = useState(value.length);
  const [emojis, setEmojis] = useState<PublicEmoji[]>([]);
  const [actors, setActors] = useState<ActorSuggestion[]>([]);
  const [knownMentions, setKnownMentions] = useState<Set<string>>(new Set());
  const [checkedMentions, setCheckedMentions] = useState<Set<string>>(
    new Set(),
  );
  const [active, setActive] = useState(-1);
  /** 候補を出しているトークンの開始位置。候補は文字を書き足したとき（`updateFromDom`で
   * 本文が伸びたとき）にだけ開き、カーソルが既存のメンション等へ入っただけでは開かない。
   * 開いた後はカーソルが同じトークンの中にある間だけ開いたままにし、Esc・候補の決定・
   * トークンから出ることで閉じる。 */
  const [suggestTokenStart, setSuggestTokenStart] = useState<number | null>(
    null,
  );

  /** 非同期に届いた結果（メンション存在確認・カスタム絵文字一覧）で装飾を更新する。
   * 更新はdangerouslySetInnerHTMLで本文DOMを作り直すので、IMEの変換中に行うと未確定の
   * 文字列がDOMごと消えて変換が壊れる。変換中は保留し、compositionendで本文を同期した
   * 後にまとめて反映する。 */
  const deferredDecorations = useRef<(() => void)[]>([]);
  function applyDecorationUpdate(update: () => void) {
    if (composing.current) {
      deferredDecorations.current.push(update);
      return;
    }
    preserveCaretForRender(editorRef.current, pendingCaret);
    update();
  }

  useEffect(() => {
    fetchCustomEmojis()
      .then((rows) => applyDecorationUpdate(() => setEmojis(rows)))
      .catch(() => setEmojis([]));
  }, []);

  useEffect(() => {
    const mentions = Array.from(new Set(value.match(MENTION_RE) ?? []));
    const unresolved = mentions.filter(
      (mention) => !checkedMentions.has(mention),
    );
    if (!unresolved.length) return;
    let cancelled = false;
    Promise.all(
      unresolved.map(async (mention) => {
        return (await mentionIsKnown(mention)) ? mention : null;
      }),
    )
      .then((resolved) => {
        if (cancelled) return;
        // 装飾クラスの更新でdangerouslySetInnerHTMLが差し替わっても、入力中の
        // caretを先頭へ飛ばさない。これが無いと逐次入力中の部分IDチェック結果が
        // 返った時点で、その後のID入力とサジェストが壊れる。
        applyDecorationUpdate(() => {
          setCheckedMentions(
            (current) => new Set([...current, ...unresolved]),
          );
          setKnownMentions(
            (current) =>
              new Set([
                ...current,
                ...resolved.filter((v): v is string => !!v),
              ]),
          );
        });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [value, checkedMentions]);

  const rawToken = value.slice(0, caret).match(TOKEN_RE)?.[0] ?? "";
  const tokenAtCaret =
    rawToken === ":" &&
    emojis.some((emoji) => value.slice(0, caret).endsWith(`:${emoji.name}:`))
      ? ""
      : rawToken;
  const tokenStart = caret - tokenAtCaret.length;
  const suggestOpen = tokenAtCaret !== "" && suggestTokenStart === tokenStart;
  /** 候補の対象トークン。候補を閉じている間は空にして、アクター検索も止める。 */
  const token = suggestOpen ? tokenAtCaret : "";
  useEffect(() => {
    if (suggestTokenStart !== null && !suggestOpen) setSuggestTokenStart(null);
  }, [suggestTokenStart, suggestOpen]);
  useEffect(() => {
    if (!token.startsWith("@")) {
      setActors([]);
      return;
    }
    const controller = new AbortController();
    const timer = window.setTimeout(() => {
      api.actors
        .suggest(token.slice(1), 8, controller.signal)
        .then(setActors)
        .catch(() => setActors([]));
    }, 200);
    return () => {
      controller.abort();
      window.clearTimeout(timer);
    };
  }, [token]);

  const candidates = useMemo<Candidate[]>(() => {
    if (token.startsWith("@")) {
      return actors.map((actor) => ({
        kind: "actor",
        key: actor.actor_id,
        value: actorValue(actor),
        actor,
      }));
    }
    if (token.startsWith(":")) {
      const query = token.slice(1).toLowerCase();
      return emojis
        .filter(
          (emoji) =>
            emoji.name.toLowerCase().includes(query) ||
            emoji.aliases.some((a) => a.toLowerCase().includes(query)),
        )
        .slice(0, 8)
        .map((emoji) => ({
          kind: "emoji",
          key: emoji.name,
          value: `:${emoji.name}:`,
          emoji,
        }));
    }
    return [];
  }, [actors, emojis, token]);

  useEffect(
    () => setActive(candidates.length ? 0 : -1),
    [token, candidates.length],
  );

  useLayoutEffect(() => {
    if (pendingCaret.current === null || !editorRef.current) return;
    restoreSelection(editorRef.current, pendingCaret.current);
    pendingCaret.current = null;
  }, [value, knownMentions, emojis, federateToBsky]);

  // Bsky配送トグルの切り替えで絵文字展開の有無が変わりDOMが作り直されるため、
  // 直前のcaret位置を保存して上のuseLayoutEffectで復元する。
  useEffect(() => {
    preserveCaretForRender(editorRef.current, pendingCaret);
  }, [federateToBsky]);

  useEffect(() => {
    if (!autoFocus || !editorRef.current) return;
    editorRef.current.focus();
    restoreSelection(editorRef.current, initialCaret.current);
  }, [autoFocus]);

  function updateFromDom() {
    const editor = editorRef.current;
    if (!editor) return;
    const nextCaret = selectionOffset(editor) ?? value.length;
    pendingCaret.current = nextCaret;
    setCaret(nextCaret);
    // 全選択削除などで内容が空になると、ブラウザはカーソル表示用の単独<br>を
    // 自動的に残す。これはユーザーが入力した改行ではないため空文字列として扱う。
    const isPlaceholderBr =
      editor.childNodes.length === 1 && editor.firstChild?.nodeName === "BR";
    const next = isPlaceholderBr ? "" : nodeValue(editor);
    if (next.length > value.length) {
      const typed = next.slice(0, nextCaret).match(TOKEN_RE)?.[0];
      setSuggestTokenStart(typed ? nextCaret - typed.length : null);
    }
    onChange(next);
  }

  function insert(candidate: Candidate) {
    const start = caret - token.length;
    const next = `${value.slice(0, start)}${candidate.value}${value.slice(caret)}`;
    const nextCaret = start + candidate.value.length;
    pendingCaret.current = nextCaret;
    setCaret(nextCaret);
    setActive(-1);
    setSuggestTokenStart(null);
    onChange(next);
    editorRef.current?.focus();
  }

  /** 絵文字・ユーザーIDピッカー（`PostComposer`）から、現在のキャレット位置へ`text`を挿入する。 */
  function insertAtCursor(text: string) {
    const { next, caret: nextCaret } = resolveInsertion(value, caret, text);
    pendingCaret.current = nextCaret;
    setCaret(nextCaret);
    onChange(next);
    editorRef.current?.focus();
  }

  // 投稿ボタン群からの矢印キーナビゲーション（上矢印で本文へフォーカスを戻す）、および
  // 絵文字・ユーザーIDピッカーからの挿入用に、本文のcontentEditable DOM要素へ
  // `insertAtCursor`を生やして親（PostComposer）へ公開する。
  useImperativeHandle(forwardedRef, () => {
    const el = editorRef.current as ComposerEditorHandle;
    if (el) el.insertAtCursor = insertAtCursor;
    return el;
  });

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    // Safariでは変換確定のEnterでcompositionendがkeydownより先に発火し、
    // composing.currentが既にfalseへ戻ってしまう。この場合もkeyCodeは229の
    // ままなので、変換確定イベントとして扱い通常のEnter処理をスキップする。
    // https://zenn.dev/spacemarket/articles/149aa284ef7b08
    if (composing.current || event.keyCode === 229) return;
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      onSubmitShortcut();
      return;
    }
    if (candidates.length && event.key === "Escape") {
      // 投稿欄を載せたモーダルもwindowでEscを拾って閉じるため、候補を閉じるだけで止める。
      event.preventDefault();
      event.stopPropagation();
      setSuggestTokenStart(null);
      return;
    }
    if (
      candidates.length &&
      ["ArrowDown", "ArrowUp", "Tab"].includes(event.key)
    ) {
      event.preventDefault();
      const direction =
        event.key === "ArrowUp" || (event.key === "Tab" && event.shiftKey)
          ? -1
          : 1;
      setActive((current) => {
        if (current < 0) return direction > 0 ? 0 : candidates.length - 1;
        return (current + direction + candidates.length) % candidates.length;
      });
      return;
    }
    if (event.key === "Enter" && active >= 0 && candidates[active]) {
      event.preventDefault();
      insert(candidates[active]);
      return;
    }
    if (event.key === "Enter") {
      // contentEditableのデフォルト改行挿入（div/br分割）に任せると、直後の
      // selectionOffset計算がbrの前後を区別できずcaretがずれる。常に自前で
      // valueへ\nを挿入することで、caret位置を正確に管理する。
      if (!window.getSelection()?.isCollapsed) return;
      event.preventDefault();
      const next = `${value.slice(0, caret)}\n${value.slice(caret)}`;
      const nextCaret = caret + 1;
      pendingCaret.current = nextCaret;
      setCaret(nextCaret);
      onChange(next);
      return;
    }
  }

  const emojiByCode = new Map(
    emojis.map((emoji) => [`:${emoji.name}:`, emoji]),
  );
  const parts = value.split(DECORATION_RE);
  // contenteditable配下をReactの子要素として管理すると、ブラウザがatomic nodeを
  // 先に編集した場合にremoveChild競合が起きる。安全にescapeしたHTMLを面全体へ
  // 一括反映し、入力値はnodeValueで常にプレーンテキストへ戻す。
  const editorHtml =
    parts
      .map((part) => {
        const emoji = emojiByCode.get(part);
        if (emoji) {
          if (federateToBsky) {
            // Bskyはカスタム絵文字を画像展開できないため、画像化せずショートコードの
            // まま警告色で表示する。編集可能なテキストとして扱うため
            // mentionKnown/mentionUnknown同様data-valueは付けない。
            return `<span class="${styles.emojiBskyWarn}" title="${escapeHtml(part)}">${escapeHtml(part)}</span>`;
          }
          return emojiImgHtml(part, emoji.url, styles.emoji);
        }
        if (part.startsWith("@")) {
          const className = knownMentions.has(part)
            ? styles.mentionKnown
            : styles.mentionUnknown;
          return `<span class="${className}">${escapeHtml(part)}</span>`;
        }
        return twemojiHtml(part, styles.emoji);
      })
      .join("") + (value.endsWith("\n") ? '<br data-pad="1">' : "");

  return (
    <div className={styles.wrap}>
      <div
        ref={editorRef}
        className={styles.editor}
        contentEditable
        role="textbox"
        aria-multiline="true"
        data-placeholder={placeholder}
        {...({ placeholder } as Record<string, string>)}
        suppressContentEditableWarning
        onInput={() => {
          // composition中に親のvalueを更新するとdangerouslySetInnerHTMLが未確定DOMを
          // 作り直し、ブラウザのIMEセッションと未確定文字列を破壊してしまう。
          // compositionendで完成したDOMを一度だけ同期する。
          if (!composing.current) updateFromDom();
        }}
        onKeyDown={handleKeyDown}
        onKeyUp={() => {
          // pendingCaret設定中（handleKeyDownで自前更新した直後）は、まだ
          // 再レンダー前の古いDOMを読むことになりcaretを巻き戻してしまうため待つ。
          if (composing.current || pendingCaret.current !== null) return;
          const next = editorRef.current
            ? selectionOffset(editorRef.current)
            : null;
          if (next !== null) setCaret(next);
        }}
        onClick={() => {
          const next = editorRef.current
            ? selectionOffset(editorRef.current)
            : null;
          if (next !== null) setCaret(next);
        }}
        onCompositionStart={() => (composing.current = true)}
        onCompositionEnd={() => {
          composing.current = false;
          updateFromDom();
          const deferred = deferredDecorations.current;
          deferredDecorations.current = [];
          deferred.forEach((update) => update());
        }}
        onPaste={(event: ClipboardEvent<HTMLDivElement>) => {
          const item = Array.from(event.clipboardData.items).find((entry) =>
            entry.type.startsWith("image/"),
          );
          const file = item?.getAsFile();
          if (file) {
            event.preventDefault();
            onImagePaste(file);
          }
        }}
        dangerouslySetInnerHTML={{ __html: editorHtml }}
      />
      {candidates.length > 0 && (
        <ul
          className={styles.suggestions}
          role="listbox"
          aria-label={t("common:inputSuggestions")}
        >
          {candidates.map((candidate, index) => (
            <li key={`${candidate.kind}-${candidate.key}`}>
              <button
                type="button"
                role="option"
                aria-selected={active === index}
                className={
                  active === index ? styles.suggestionActive : styles.suggestion
                }
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => insert(candidate)}
              >
                {candidate.kind === "emoji" ? (
                  <>
                    <img src={candidate.emoji.url} alt="" />:
                    {candidate.emoji.name}:
                  </>
                ) : (
                  <>
                    {candidate.actor.avatar_url && (
                      <RemoteImg src={candidate.actor.avatar_url} alt="" />
                    )}
                    <span>
                      <DisplayName name={candidate.actor.display_name} fallback={candidate.actor.username} emojis={candidate.actor.emojis} />
                    </span>
                    <small>{candidate.value}</small>
                  </>
                )}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
});

export default ComposerEditor;

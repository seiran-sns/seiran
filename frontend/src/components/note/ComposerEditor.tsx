import {
  ClipboardEvent,
  KeyboardEvent,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { ActorSuggestion, PublicEmoji, api } from "../../api/client";
import { fetchCustomEmojis } from "../../lib/customEmojis";
import styles from "./ComposerEditor.module.css";

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
}

const MENTION_RE = /(?<![\w])@[A-Za-z0-9_-]+(?:\.[A-Za-z0-9-]+)*(?:@[A-Za-z0-9.-]+)?/g;
const TOKEN_RE = /(?<![\w])@[A-Za-z0-9_.@-]*$|:[A-Za-z0-9_+-]*$/;

function nodeValue(node: Node): string {
  if (node instanceof HTMLElement && node.dataset.value) return node.dataset.value;
  if (node.nodeType === Node.TEXT_NODE) return node.textContent ?? "";
  if (node.nodeName === "BR") return "\n";
  return Array.from(node.childNodes).map(nodeValue).join("");
}

function selectionOffset(root: HTMLElement): number | null {
  const selection = window.getSelection();
  if (!selection?.rangeCount || !root.contains(selection.anchorNode)) return null;
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

function actorValue(actor: ActorSuggestion) {
  if (actor.actor_type === "local") return `@${actor.username}`;
  return actor.domain ? `@${actor.username}@${actor.domain}` : `@${actor.username}`;
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
  if (!target.includes("@") && target.includes(".")) queries.push(target.slice(0, target.indexOf(".")));
  const rows = (await Promise.all(queries.map((query) => api.actors.search(query, 10)))).flat();
  return rows.some((row) =>
    actorMentionValues(row).some((candidate) => candidate.toLowerCase() === mention.toLowerCase())
  );
}

function escapeHtml(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

export default function ComposerEditor({
  value,
  onChange,
  onSubmitShortcut,
  onImagePaste,
  placeholder,
  autoFocus,
}: ComposerEditorProps) {
  const editorRef = useRef<HTMLDivElement>(null);
  const pendingCaret = useRef<number | null>(null);
  const composing = useRef(false);
  const initialCaret = useRef(value.length);
  const [caret, setCaret] = useState(value.length);
  const [emojis, setEmojis] = useState<PublicEmoji[]>([]);
  const [actors, setActors] = useState<ActorSuggestion[]>([]);
  const [knownMentions, setKnownMentions] = useState<Set<string>>(new Set());
  const [checkedMentions, setCheckedMentions] = useState<Set<string>>(new Set());
  const [active, setActive] = useState(-1);

  useEffect(() => {
    fetchCustomEmojis().then(setEmojis).catch(() => setEmojis([]));
  }, []);

  useEffect(() => {
    const mentions = Array.from(new Set(value.match(MENTION_RE) ?? []));
    const unresolved = mentions.filter((mention) => !checkedMentions.has(mention));
    if (!unresolved.length) return;
    let cancelled = false;
    Promise.all(
      unresolved.map(async (mention) => {
        return (await mentionIsKnown(mention)) ? mention : null;
      })
    )
      .then((resolved) => {
        if (cancelled) return;
        setCheckedMentions((current) => new Set([...current, ...unresolved]));
        setKnownMentions((current) => new Set([...current, ...resolved.filter((v): v is string => !!v)]));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [value, checkedMentions]);

  const rawToken = value.slice(0, caret).match(TOKEN_RE)?.[0] ?? "";
  const token =
    rawToken === ":" &&
    emojis.some((emoji) => value.slice(0, caret).endsWith(`:${emoji.name}:`))
      ? ""
      : rawToken;
  useEffect(() => {
    if (!token.startsWith("@")) {
      setActors([]);
      return;
    }
    const controller = new AbortController();
    const timer = window.setTimeout(() => {
      api.actors
        .search(token.slice(1), 8, controller.signal)
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
        .filter((emoji) => emoji.name.toLowerCase().includes(query) || emoji.aliases.some((a) => a.toLowerCase().includes(query)))
        .slice(0, 8)
        .map((emoji) => ({ kind: "emoji", key: emoji.name, value: `:${emoji.name}:`, emoji }));
    }
    return [];
  }, [actors, emojis, token]);

  useEffect(() => setActive(candidates.length ? 0 : -1), [token, candidates.length]);

  useLayoutEffect(() => {
    if (pendingCaret.current === null || !editorRef.current) return;
    restoreSelection(editorRef.current, pendingCaret.current);
    pendingCaret.current = null;
  }, [value, knownMentions, emojis]);

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
    onChange(nodeValue(editor).replace(/\n$/, ""));
  }

  function insert(candidate: Candidate) {
    const start = caret - token.length;
    const next = `${value.slice(0, start)}${candidate.value}${value.slice(caret)}`;
    const nextCaret = start + candidate.value.length;
    pendingCaret.current = nextCaret;
    setCaret(nextCaret);
    setActive(-1);
    onChange(next);
    editorRef.current?.focus();
  }

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (composing.current) return;
    if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
      event.preventDefault();
      onSubmitShortcut();
      return;
    }
    if (candidates.length && ["ArrowDown", "ArrowUp", "Tab"].includes(event.key)) {
      event.preventDefault();
      const direction = event.key === "ArrowUp" || (event.key === "Tab" && event.shiftKey) ? -1 : 1;
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
    if (event.key === "Backspace" && !window.getSelection()?.isCollapsed) return;
    if (event.key === "Backspace") {
      const emoji = emojis.find((item) => {
        const shortcode = `:${item.name}:`;
        return value.slice(0, caret).endsWith(shortcode) || value.slice(caret).startsWith(shortcode);
      });
      if (emoji) {
        event.preventDefault();
        const shortcode = `:${emoji.name}:`;
        const start = value.slice(0, caret).endsWith(shortcode) ? caret - shortcode.length : caret;
        const removeAt = start === caret ? start : start + shortcode.length - 1;
        pendingCaret.current = removeAt;
        setCaret(removeAt);
        onChange(value.slice(0, removeAt) + value.slice(removeAt + 1));
      }
    }
  }

  const emojiByCode = new Map(emojis.map((emoji) => [`:${emoji.name}:`, emoji]));
  const parts = value.split(/(:[A-Za-z0-9_+-]+:|(?<![\w])@[A-Za-z0-9_-]+(?:\.[A-Za-z0-9-]+)*(?:@[A-Za-z0-9.-]+)?)/g);
  // contenteditable配下をReactの子要素として管理すると、ブラウザがatomic nodeを
  // 先に編集した場合にremoveChild競合が起きる。安全にescapeしたHTMLを面全体へ
  // 一括反映し、入力値はnodeValueで常にプレーンテキストへ戻す。
  const editorHtml = parts
    .map((part) => {
      const emoji = emojiByCode.get(part);
      if (emoji) {
        return `<span class="${styles.emoji}" contenteditable="false" data-value="${escapeHtml(part)}"><img src="${escapeHtml(emoji.url)}" alt="${escapeHtml(part)}" title="${escapeHtml(part)}"></span>`;
      }
      if (part.startsWith("@")) {
        const className = knownMentions.has(part) ? styles.mentionKnown : styles.mentionUnknown;
        return `<span class="${className}">${escapeHtml(part)}</span>`;
      }
      return escapeHtml(part).replace(/\n/g, "<br>");
    })
    .join("");

  return (
    <div className={styles.wrap}>
      {!value && <span className={styles.placeholder}>{placeholder}</span>}
      <div
        ref={editorRef}
        className={styles.editor}
        contentEditable
        role="textbox"
        aria-multiline="true"
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
          if (composing.current) return;
          const next = editorRef.current ? selectionOffset(editorRef.current) : null;
          if (next !== null) setCaret(next);
        }}
        onClick={() => {
          const next = editorRef.current ? selectionOffset(editorRef.current) : null;
          if (next !== null) setCaret(next);
        }}
        onCompositionStart={() => (composing.current = true)}
        onCompositionEnd={() => {
          composing.current = false;
          updateFromDom();
        }}
        onPaste={(event: ClipboardEvent<HTMLDivElement>) => {
          const item = Array.from(event.clipboardData.items).find((entry) => entry.type.startsWith("image/"));
          const file = item?.getAsFile();
          if (file) {
            event.preventDefault();
            onImagePaste(file);
          }
        }}
        dangerouslySetInnerHTML={{ __html: editorHtml }}
      />
      {candidates.length > 0 && (
        <ul className={styles.suggestions} role="listbox" aria-label="入力候補">
          {candidates.map((candidate, index) => (
            <li key={`${candidate.kind}-${candidate.key}`}>
              <button
                type="button"
                role="option"
                aria-selected={active === index}
                className={active === index ? styles.suggestionActive : styles.suggestion}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => insert(candidate)}
              >
                {candidate.kind === "emoji" ? (
                  <>
                    <img src={candidate.emoji.url} alt="" />
                    :{candidate.emoji.name}:
                  </>
                ) : (
                  <>
                    {candidate.actor.avatar_url && <img src={candidate.actor.avatar_url} alt="" />}
                    <span>{candidate.actor.display_name || candidate.actor.username}</span>
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
}

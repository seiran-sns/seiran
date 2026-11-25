import { type JSX, RefObject, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, FrequentReaction, PublicEmoji } from "../../api/client";
import { useLazyVisible } from "../../hooks/useLazyVisible";
import { postLanguageBase } from "../../i18n";
import {
  fetchCustomEmojis,
  isLocalCustomEmoji,
  parseCustomEmojiShortcode,
  parseReactionContent,
} from "../../lib/customEmojis";
import { EmojiAnnotationIndex, loadEmojiAnnotationIndex } from "../../lib/emojiAnnotations";
import { unicodeEmojiGroups } from "../../lib/emojiData";
import { currentUtcOffsetMinutes } from "../../lib/reactionTimeSlot";
import { emojiAspectSpan } from "../../lib/emojiAspect";
import TwemojiEmoji from "../common/TwemojiEmoji";
import EmojiImage from "./EmojiImage";
import styles from "./EmojiPickerPanel.module.css";

type Tab = "frequent" | "unicode" | "custom";

interface PickerItem {
  key: string;
  /** リアクションとして送信する値（Unicode絵文字文字列 or `:shortcode:`）。 */
  content: string;
  /** 検索対象・alt/title 文字列。 */
  label: string;
  imageUrl?: string;
  /** 画像の実寸・プレースホルダ（`imageUrl`がある場合のみ意味を持つ）。 */
  width?: number;
  height?: number;
  blurhash?: string;
  /** カスタム絵文字のカテゴリ（タブ内でのセクション分け用）。未分類は `null`。 */
  category?: string | null;
}

interface PickerGroup {
  /** カスタムとUnicodeで同名のカテゴリがありうるため、種別を前置した識別子。 */
  key: string;
  name: string;
  items: PickerItem[];
}

interface EmojiPickerPanelProps {
  onPick: (content: string) => void;
  /** Bsky宛DMリアクション等、Unicode絵文字のみ選択可能にしたい場合に指定する。
   * カスタム絵文字タブを隠し、よく使う絵文字タブからもカスタム絵文字を除外する。 */
  unicodeOnly?: boolean;
}

const SEARCH_RESULT_LIMIT = 100;

/** 検索結果がこの件数以上なら、タブと同じカテゴリ別のたためるセクションに分けて出す。 */
const SEARCH_GROUPING_THRESHOLD = 30;

/**
 * カスタム絵文字が数千件規模になりうる一覧・検索結果で一度に描画する button DOM の初期件数、
 * および末尾のセンチネルが可視になるたびに追加する件数。全件を一度に `.map` すると、
 * 特にサーバー到達後はじめて「カスタム」タブを開いた際に描画だけで数秒固まる。
 */
const GRID_PAGE_SIZE = 200;

interface LoadMoreSentinelProps {
  rootRef: RefObject<Element | null>;
  onVisible: () => void;
}

/** `rootRef` のスクロールコンテナ内で可視になるたびに `onVisible` を呼ぶ、高さ1pxの監視用要素。 */
function LoadMoreSentinel({ rootRef, onVisible }: LoadMoreSentinelProps) {
  const ref = useRef<HTMLDivElement>(null);
  const visible = useLazyVisible(ref, rootRef, "300px");
  useEffect(() => {
    if (visible) onVisible();
  }, [visible, onVisible]);
  return <div ref={ref} className={styles.sentinel} aria-hidden="true" />;
}

interface PagedGridProps {
  items: PickerItem[];
  rootRef: RefObject<Element | null>;
  renderItem: (item: PickerItem) => JSX.Element;
}

/**
 * `items` を `GRID_PAGE_SIZE` 件ずつ段階的に描画するグリッド。`items` の参照が変わると
 * （検索クエリの変更、タブ切り替えによる再マウント）表示件数を初期値に戻す。
 */
function PagedGrid({ items, rootRef, renderItem }: PagedGridProps) {
  const [visibleCount, setVisibleCount] = useState(GRID_PAGE_SIZE);
  useEffect(() => {
    setVisibleCount(GRID_PAGE_SIZE);
  }, [items]);

  return (
    <div className={styles.grid}>
      {items.slice(0, visibleCount).map(renderItem)}
      {visibleCount < items.length && (
        <LoadMoreSentinel
          rootRef={rootRef}
          onVisible={() => setVisibleCount((c) => Math.min(c + GRID_PAGE_SIZE, items.length))}
        />
      )}
    </div>
  );
}

/**
 * 1行に並びうる件数の上限より多い件数。たたんだセクションはこれだけ描画して1行分の高さで切る
 * （全件を描画したまま隠すと、数千件のカスタム絵文字で描画が固まる）。1カラム36pxなので、
 * ピッカーがこの幅（1,440px）を超えない限り1行目が欠けない。
 */
const COLLAPSED_ITEM_LIMIT = 40;

interface CollapsibleGroupProps {
  group: PickerGroup;
  rootRef: RefObject<Element | null>;
  renderItem: (item: PickerItem) => JSX.Element;
}

/**
 * カテゴリ1つ分のセクション。初めは1行分だけ見せ、収まらなければ行末に「全部」ボタンを出す。
 * 押すと全件（`PagedGrid` で段階的に）を出し、ボタンは「たたむ」に変わる。
 */
function CollapsibleGroup({ group, rootRef, renderItem }: CollapsibleGroupProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const [overflow, setOverflow] = useState(group.items.length > COLLAPSED_ITEM_LIMIT);
  const groupRef = useRef<HTMLDivElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);

  // たたんだグリッドは高さ1行で切っているので、中身がそれより高ければ収まっていない。
  // 1行に入る件数はピッカーの幅とセル幅（カスタム絵文字は1〜4カラム）で決まるため実測する。
  useLayoutEffect(() => {
    if (expanded) return;
    const el = gridRef.current;
    if (!el) return;
    const measure = () =>
      setOverflow(group.items.length > COLLAPSED_ITEM_LIMIT || el.scrollHeight > el.clientHeight + 1);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [expanded, group.items]);

  function collapse() {
    setExpanded(false);
    // 長いセクションの下の方でたたむと、見ていた位置ごと中身が消えて後ろのセクションに飛ぶため、
    // たたんだセクションを見える所に戻す。
    requestAnimationFrame(() => groupRef.current?.scrollIntoView({ block: "nearest" }));
  }

  return (
    <div ref={groupRef} className={styles.group}>
      <div className={styles.groupTitle}>{group.name}</div>
      <div className={styles.groupRow}>
        <div className={styles.groupGrid}>
          {expanded ? (
            <PagedGrid items={group.items} rootRef={rootRef} renderItem={renderItem} />
          ) : (
            <div ref={gridRef} className={`${styles.grid} ${styles.gridCollapsed}`}>
              {group.items.slice(0, COLLAPSED_ITEM_LIMIT).map(renderItem)}
            </div>
          )}
        </div>
        {(expanded || overflow) && (
          <button
            type="button"
            className={styles.groupToggle}
            aria-expanded={expanded}
            onClick={(e) => {
              e.stopPropagation();
              if (expanded) collapse();
              else setExpanded(true);
            }}
          >
            {expanded ? t("home:reactionPicker.collapse") : t("home:reactionPicker.showAll")}
          </button>
        )}
      </div>
    </div>
  );
}

interface PagedGroupListProps {
  groups: PickerGroup[];
  rootRef: RefObject<Element | null>;
  renderItem: (item: PickerItem) => JSX.Element;
}

/**
 * `groups` をたためるセクションとして並べる。カテゴリが多いと、たたんだ1行分ずつでも全セクションを
 * 一度に描画すると固まるため、たたんだ状態で描く件数の合計が `GRID_PAGE_SIZE` に達するまでの
 * セクションを描き、末尾のセンチネルが見えるたびに同じ件数分のセクションを足す。
 */
function PagedGroupList({ groups, rootRef, renderItem }: PagedGroupListProps) {
  const nonEmpty = useMemo(() => groups.filter((g) => g.items.length > 0), [groups]);
  const [budget, setBudget] = useState(GRID_PAGE_SIZE);
  useEffect(() => {
    setBudget(GRID_PAGE_SIZE);
  }, [groups]);

  let shownGroups = 0;
  let spent = 0;
  while (shownGroups < nonEmpty.length && spent < budget) {
    spent += Math.min(nonEmpty[shownGroups].items.length, COLLAPSED_ITEM_LIMIT);
    shownGroups++;
  }

  return (
    <>
      {nonEmpty.slice(0, shownGroups).map((group) => (
        <CollapsibleGroup key={group.key} group={group} rootRef={rootRef} renderItem={renderItem} />
      ))}
      {shownGroups < nonEmpty.length && (
        <LoadMoreSentinel rootRef={rootRef} onVisible={() => setBudget((b) => b + GRID_PAGE_SIZE)} />
      )}
    </>
  );
}

/**
 * カスタム絵文字をカテゴリ別のセクションに分ける。カテゴリ名の昇順で、未設定は末尾の
 * `uncategorizedLabel` セクションにまとめる。
 */
function groupCustomItems(items: PickerItem[], uncategorizedLabel: string, locale: string): PickerGroup[] {
  const byCategory = new Map<string, PickerItem[]>();
  const uncategorized: PickerItem[] = [];
  for (const item of items) {
    const category = item.category?.trim();
    if (!category) {
      uncategorized.push(item);
      continue;
    }
    const bucket = byCategory.get(category);
    if (bucket) bucket.push(item);
    else byCategory.set(category, [item]);
  }
  const groups: PickerGroup[] = [...byCategory.entries()]
    .sort(([a], [b]) => a.localeCompare(b, locale))
    .map(([name, groupItems]) => ({ key: `c:${name}`, name, items: groupItems }));
  if (uncategorized.length > 0) {
    groups.push({ key: "c:", name: uncategorizedLabel, items: uncategorized });
  }
  return groups;
}

const unicodeGroups: PickerGroup[] = unicodeEmojiGroups.map((group) => ({
  key: `u:${group.name}`,
  name: group.name,
  items: group.emojis.map((e) => ({ key: `u:${e.emoji}`, content: e.emoji, label: e.name })),
}));

/** カスタム絵文字＋Unicode絵文字を検索・タブ切り替えで選べるピッカー本体（Modal 内に描画する）。 */
export default function EmojiPickerPanel({ onPick, unicodeOnly = false }: EmojiPickerPanelProps) {
  const { t, i18n } = useTranslation();
  const [customEmojis, setCustomEmojis] = useState<PublicEmoji[]>([]);
  const [frequent, setFrequent] = useState<FrequentReaction[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [tab, setTab] = useState<Tab>("unicode");
  const [annotations, setAnnotations] = useState<EmojiAnnotationIndex>(new Map());
  const bodyRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    // `i18n.language` は検出された生の言語コード（例: "ja-JP"）を返すことがあるため、
    // `displayLanguages` と同じ表記に解決済みの `resolvedLanguage` を優先する。絵文字
    // アノテーションデータは表示言語と異なり中国語のバリエーションを持たないため、
    // `postLanguageBase` で `zh-Hant`/`zh-Hans` を `zh` に丸める。
    const detected = i18n.resolvedLanguage ?? i18n.language;
    const uiLanguage = postLanguageBase(detected);
    let cancelled = false;
    loadEmojiAnnotationIndex(uiLanguage).then((index) => {
      if (!cancelled) setAnnotations(index);
    });
    return () => {
      cancelled = true;
    };
  }, [i18n.language, i18n.resolvedLanguage]);

  useEffect(() => {
    if (unicodeOnly && tab === "custom") setTab("unicode");
  }, [unicodeOnly, tab]);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      fetchCustomEmojis().catch(() => [] as PublicEmoji[]),
      api.reactions
        .frequent(currentUtcOffsetMinutes())
        .catch(() => ({ items: [] as FrequentReaction[] })),
    ]).then(([emojis, frequentRes]) => {
      if (cancelled) return;
      setCustomEmojis(emojis);
      setFrequent(frequentRes.items);
      if (frequentRes.items.length > 0) setTab("frequent");
      setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const customItems: PickerItem[] = useMemo(
    () =>
      customEmojis.map((e) => ({
        key: `custom:${e.name}`,
        content: `:${e.name}:`,
        label: [e.name, ...e.aliases].join(" "),
        imageUrl: e.url,
        width: e.width,
        height: e.height,
        blurhash: e.blurhash,
        category: e.category,
      })),
    [customEmojis]
  );

  const customGroups: PickerGroup[] = useMemo(
    () => groupCustomItems(customItems, t("home:reactionPicker.uncategorized"), i18n.language),
    [customItems, i18n.language, t]
  );

  const frequentItems: PickerItem[] = useMemo(() => {
    // `f.content` はDBの生content（`:shortcode@.:` 等ホスト付きになりうる）なので、shortcode
    // 部分だけを取り出してローカルカスタム絵文字一覧（host無し `:shortcode:` キー）と照合する。
    const customByShortcode = new Map(
      customItems.map((i) => [parseCustomEmojiShortcode(i.content), i])
    );
    return frequent
      .filter((f) => !unicodeOnly || parseReactionContent(f.content) === null)
      .map((f) => {
        const parsed = parseReactionContent(f.content);
        const custom =
          parsed && isLocalCustomEmoji(parsed) ? customByShortcode.get(parsed.shortcode) : undefined;
        if (custom) return custom;
        return { key: `frequent:${f.content}`, content: f.content, label: f.content, imageUrl: f.emojiUrl ?? undefined };
      });
  }, [frequent, customItems, unicodeOnly]);

  const searchResults: { items: PickerItem[]; groups: PickerGroup[] | null } | null = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return null;
    const customMatches = unicodeOnly ? [] : customItems.filter((i) => i.label.toLowerCase().includes(q));
    // Unicode 側は件数を絞りつつ、どのグループの一致かを保ったまま集める。
    let unicodeBudget = SEARCH_RESULT_LIMIT;
    const unicodeMatchGroups: PickerGroup[] = [];
    for (const group of unicodeGroups) {
      if (unicodeBudget <= 0) break;
      const matched = group.items
        .filter((item) => {
          if (item.content.includes(q)) return true;
          if (item.label.toLowerCase().includes(q)) return true;
          const words = annotations.get(item.content);
          return words?.some((w) => w.toLowerCase().includes(q)) ?? false;
        })
        .slice(0, unicodeBudget);
      unicodeBudget -= matched.length;
      if (matched.length > 0) unicodeMatchGroups.push({ ...group, items: matched });
    }
    const items = [...customMatches, ...unicodeMatchGroups.flatMap((g) => g.items)];
    const groups =
      items.length >= SEARCH_GROUPING_THRESHOLD
        ? [
            ...groupCustomItems(customMatches, t("home:reactionPicker.uncategorized"), i18n.language),
            ...unicodeMatchGroups,
          ]
        : null;
    return { items, groups };
  }, [query, customItems, annotations, unicodeOnly, t, i18n.language]);

  function renderItem(item: PickerItem) {
    const span = item.imageUrl ? emojiAspectSpan(item.width, item.height) : 1;
    return (
      <button
        key={item.key}
        type="button"
        className={`${styles.item} ${styles[`span${span}`]}`}
        data-picker-item=""
        title={item.label}
        onClick={(e) => {
          e.stopPropagation();
          onPick(item.content);
        }}
      >
        {item.imageUrl ? (
          <EmojiImage src={item.imageUrl} alt={item.label} blurhash={item.blurhash} span={span} rootRef={bodyRef} />
        ) : (
          <TwemojiEmoji emoji={item.content} className={styles.itemImg} />
        )}
      </button>
    );
  }

  /**
   * いま操作できる絵文字ボタン（文書順）。たたんだセクションで1行目より後ろに隠れている分は
   * 描画されていても見えないので除く。
   */
  function visibleItemButtons(): HTMLElement[] {
    const body = bodyRef.current;
    if (!body) return [];
    return Array.from(body.querySelectorAll<HTMLElement>("[data-picker-item]")).filter((el) => {
      const clip = el.closest(`.${styles.gridCollapsed}`);
      if (!clip) return true;
      return el.getBoundingClientRect().top < clip.getBoundingClientRect().bottom - 1;
    });
  }

  /**
   * 上下矢印の移動先。横長のカスタム絵文字があり1行の件数が行ごとに違うため、添字ではなく画面上の
   * 位置で決める: 真下（真上）の行のうち、横方向の中心がいちばん近いボタン。無ければ `null`。
   */
  function verticalNeighbor(buttons: HTMLElement[], current: HTMLElement, down: boolean): HTMLElement | null {
    const cur = current.getBoundingClientRect();
    const centerX = cur.left + cur.width / 2;
    const rects = buttons.map((el) => ({ el, rect: el.getBoundingClientRect() }));
    const beyond = rects.filter(({ rect }) =>
      down ? rect.top >= cur.bottom - 2 : rect.bottom <= cur.top + 2
    );
    if (beyond.length === 0) return null;
    // 隣の行 = 下なら上端がいちばん上、上なら下端がいちばん下のボタンと同じ行。
    const rowEdge = down
      ? Math.min(...beyond.map(({ rect }) => rect.top))
      : Math.max(...beyond.map(({ rect }) => rect.bottom));
    const row = beyond.filter(({ rect }) =>
      Math.abs((down ? rect.top : rect.bottom) - rowEdge) < cur.height / 2
    );
    let best = row[0];
    for (const candidate of row) {
      const distance = Math.abs(candidate.rect.left + candidate.rect.width / 2 - centerX);
      if (distance < Math.abs(best.rect.left + best.rect.width / 2 - centerX)) best = candidate;
    }
    return best.el;
  }

  /**
   * 絵文字の上の矢印キー（Slack と同じ）。左右は前後の絵文字へ（先頭で左は末尾へ、末尾で右は
   * 先頭へ回る）、上下は上下の行の絵文字へ。いちばん上の行で上を押すと検索欄へ戻る。
   * 一覧のスクロールは、フォーカスした絵文字が見えるようブラウザが合わせる。
   */
  function handleBodyKeyDown(e: React.KeyboardEvent<HTMLDivElement>) {
    if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(e.key)) return;
    const current = (e.target as HTMLElement).closest<HTMLElement>("[data-picker-item]");
    if (!current) return;
    const buttons = visibleItemButtons();
    const index = buttons.indexOf(current);
    if (index < 0) return;
    e.preventDefault();
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      const step = e.key === "ArrowRight" ? 1 : -1;
      buttons[(index + step + buttons.length) % buttons.length].focus();
      return;
    }
    const next = verticalNeighbor(buttons, current, e.key === "ArrowDown");
    if (next) next.focus();
    else if (e.key === "ArrowUp") searchRef.current?.focus();
  }

  /**
   * 検索欄の下矢印、または末尾にカーソルがあるときの右矢印で、先頭の絵文字へ移る。
   * IME の変換中は何もしない（ATOK では下矢印が注目文節の確定など、変換操作に使われる）。
   */
  function handleSearchKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key !== "ArrowRight" && e.key !== "ArrowDown") return;
    // 変換中のキーは isComposing、変換確定直後のキーは keyCode 229 で来るブラウザがある。
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    if (e.key === "ArrowRight") {
      const input = e.currentTarget;
      const end = input.value.length;
      if (input.selectionStart !== end || input.selectionEnd !== end) return;
    }
    const first = visibleItemButtons()[0];
    if (!first) return;
    e.preventDefault();
    first.focus();
  }

  return (
    <div className={styles.wrap}>
      <input
        ref={searchRef}
        type="text"
        className={styles.search}
        placeholder={t("home:reactionPicker.searchPlaceholder")}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={handleSearchKeyDown}
        autoFocus
      />

      {!query.trim() && (
        <div className={styles.tabs}>
          <button
            type="button"
            className={`${styles.tab} ${tab === "frequent" ? styles.tabActive : ""}`}
            onClick={() => setTab("frequent")}
          >
            {t("home:reactionPicker.tabFrequent")}
          </button>
          <button
            type="button"
            className={`${styles.tab} ${tab === "unicode" ? styles.tabActive : ""}`}
            onClick={() => setTab("unicode")}
          >
            {t("home:reactionPicker.tabUnicode")}
          </button>
          {!unicodeOnly && (
            <button
              type="button"
              className={`${styles.tab} ${tab === "custom" ? styles.tabActive : ""}`}
              onClick={() => setTab("custom")}
            >
              {t("home:reactionPicker.tabCustom")}
            </button>
          )}
        </div>
      )}

      <div className={styles.body} ref={bodyRef} onKeyDown={handleBodyKeyDown}>
        {loading ? (
          <p className={styles.message}>{t("common:loading")}</p>
        ) : query.trim() ? (
          searchResults && searchResults.groups ? (
            <PagedGroupList groups={searchResults.groups} rootRef={bodyRef} renderItem={renderItem} />
          ) : searchResults && searchResults.items.length > 0 ? (
            <PagedGrid items={searchResults.items} rootRef={bodyRef} renderItem={renderItem} />
          ) : (
            <p className={styles.message}>{t("home:reactionPicker.noResults")}</p>
          )
        ) : tab === "frequent" ? (
          frequentItems.length > 0 ? (
            <div className={styles.grid}>{frequentItems.map(renderItem)}</div>
          ) : (
            <p className={styles.message}>{t("home:reactionPicker.noFrequent")}</p>
          )
        ) : tab === "custom" ? (
          customItems.length > 0 ? (
            <PagedGroupList groups={customGroups} rootRef={bodyRef} renderItem={renderItem} />
          ) : (
            <p className={styles.message}>{t("home:reactionPicker.noCustomEmojis")}</p>
          )
        ) : (
          <PagedGroupList groups={unicodeGroups} rootRef={bodyRef} renderItem={renderItem} />
        )}
      </div>
    </div>
  );
}

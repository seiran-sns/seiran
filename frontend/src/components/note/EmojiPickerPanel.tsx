import { RefObject, useEffect, useMemo, useRef, useState } from "react";
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
import { allUnicodeEmojis, unicodeEmojiGroups } from "../../lib/emojiData";
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
}

interface EmojiPickerPanelProps {
  onPick: (content: string) => void;
}

const SEARCH_RESULT_LIMIT = 100;

/**
 * カスタム絵文字が数千件規模になりうる一覧・検索結果で一度に描画する button DOM の初期件数、
 * および末尾のセンチネルが可視になるたびに追加する件数。全件を一度に `.map` すると、
 * 特にサーバー到達後はじめて「カスタム」タブを開いた際に描画だけで数秒固まる原因になっていた。
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

/** カスタム絵文字＋Unicode絵文字を検索・タブ切り替えで選べるピッカー本体（Modal 内に描画する）。 */
export default function EmojiPickerPanel({ onPick }: EmojiPickerPanelProps) {
  const { t, i18n } = useTranslation();
  const [customEmojis, setCustomEmojis] = useState<PublicEmoji[]>([]);
  const [frequent, setFrequent] = useState<FrequentReaction[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [tab, setTab] = useState<Tab>("unicode");
  const [annotations, setAnnotations] = useState<EmojiAnnotationIndex>(new Map());
  const bodyRef = useRef<HTMLDivElement>(null);

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
    let cancelled = false;
    Promise.all([
      fetchCustomEmojis().catch(() => [] as PublicEmoji[]),
      api.reactions.frequent().catch(() => ({ items: [] as FrequentReaction[] })),
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
      })),
    [customEmojis]
  );

  const frequentItems: PickerItem[] = useMemo(() => {
    // `f.content` はDBの生content（`:shortcode@.:` 等ホスト付きになりうる）なので、shortcode
    // 部分だけを取り出してローカルカスタム絵文字一覧（host無し `:shortcode:` キー）と照合する。
    const customByShortcode = new Map(
      customItems.map((i) => [parseCustomEmojiShortcode(i.content), i])
    );
    return frequent.map((f) => {
      const parsed = parseReactionContent(f.content);
      const custom =
        parsed && isLocalCustomEmoji(parsed) ? customByShortcode.get(parsed.shortcode) : undefined;
      if (custom) return custom;
      return { key: `frequent:${f.content}`, content: f.content, label: f.content, imageUrl: f.emojiUrl ?? undefined };
    });
  }, [frequent, customItems]);

  const searchResults: PickerItem[] | null = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return null;
    const customMatches = customItems.filter((i) => i.label.toLowerCase().includes(q));
    const unicodeMatches: PickerItem[] = allUnicodeEmojis
      .filter((e) => {
        if (e.emoji.includes(q)) return true;
        if (e.name.toLowerCase().includes(q)) return true;
        const words = annotations.get(e.emoji);
        return words?.some((w) => w.toLowerCase().includes(q)) ?? false;
      })
      .slice(0, SEARCH_RESULT_LIMIT)
      .map((e) => ({ key: `u:${e.emoji}`, content: e.emoji, label: e.name }));
    return [...customMatches, ...unicodeMatches];
  }, [query, customItems, annotations]);

  function renderItem(item: PickerItem) {
    const span = item.imageUrl ? emojiAspectSpan(item.width, item.height) : 1;
    return (
      <button
        key={item.key}
        type="button"
        className={`${styles.item} ${styles[`span${span}`]}`}
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

  return (
    <div className={styles.wrap}>
      <input
        type="text"
        className={styles.search}
        placeholder={t("home:reactionPicker.searchPlaceholder")}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
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
          <button
            type="button"
            className={`${styles.tab} ${tab === "custom" ? styles.tabActive : ""}`}
            onClick={() => setTab("custom")}
          >
            {t("home:reactionPicker.tabCustom")}
          </button>
        </div>
      )}

      <div className={styles.body} ref={bodyRef}>
        {loading ? (
          <p className={styles.message}>{t("common:loading")}</p>
        ) : query.trim() ? (
          searchResults && searchResults.length > 0 ? (
            <PagedGrid items={searchResults} rootRef={bodyRef} renderItem={renderItem} />
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
            <PagedGrid items={customItems} rootRef={bodyRef} renderItem={renderItem} />
          ) : (
            <p className={styles.message}>{t("home:reactionPicker.noCustomEmojis")}</p>
          )
        ) : (
          unicodeEmojiGroups.map((group) => (
            <div key={group.name} className={styles.group}>
              <div className={styles.groupTitle}>{group.name}</div>
              <div className={styles.grid}>
                {group.emojis.map((e) => renderItem({ key: `u:${e.emoji}`, content: e.emoji, label: e.name }))}
              </div>
            </div>
          ))
        )}
      </div>
    </div>
  );
}

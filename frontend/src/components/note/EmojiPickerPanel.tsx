import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, FrequentReaction, PublicEmoji } from "../../api/client";
import { allUnicodeEmojis, unicodeEmojiGroups } from "../../lib/emojiData";
import styles from "./EmojiPickerPanel.module.css";

type Tab = "frequent" | "unicode" | "custom";

interface PickerItem {
  key: string;
  /** リアクションとして送信する値（Unicode絵文字文字列 or `:shortcode:`）。 */
  content: string;
  /** 検索対象・alt/title 文字列。 */
  label: string;
  imageUrl?: string;
}

interface EmojiPickerPanelProps {
  onPick: (content: string) => void;
}

const SEARCH_RESULT_LIMIT = 100;

/** カスタム絵文字＋Unicode絵文字を検索・タブ切り替えで選べるピッカー本体（Modal 内に描画する）。 */
export default function EmojiPickerPanel({ onPick }: EmojiPickerPanelProps) {
  const { t } = useTranslation();
  const [customEmojis, setCustomEmojis] = useState<PublicEmoji[]>([]);
  const [frequent, setFrequent] = useState<FrequentReaction[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [tab, setTab] = useState<Tab>("unicode");

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      api.emojis.list().catch(() => ({ emojis: [] as PublicEmoji[] })),
      api.reactions.frequent().catch(() => ({ items: [] as FrequentReaction[] })),
    ]).then(([emojisRes, frequentRes]) => {
      if (cancelled) return;
      setCustomEmojis(emojisRes.emojis);
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
      })),
    [customEmojis]
  );

  const frequentItems: PickerItem[] = useMemo(() => {
    const customByContent = new Map(customItems.map((i) => [i.content, i]));
    return frequent.map((f) => {
      const custom = customByContent.get(f.content);
      if (custom) return custom;
      return { key: `frequent:${f.content}`, content: f.content, label: f.content, imageUrl: f.emojiUrl ?? undefined };
    });
  }, [frequent, customItems]);

  const searchResults: PickerItem[] | null = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return null;
    const customMatches = customItems.filter((i) => i.label.toLowerCase().includes(q));
    const unicodeMatches: PickerItem[] = allUnicodeEmojis
      .filter((e) => e.name.toLowerCase().includes(q))
      .slice(0, SEARCH_RESULT_LIMIT)
      .map((e) => ({ key: `u:${e.emoji}`, content: e.emoji, label: e.name }));
    return [...customMatches, ...unicodeMatches];
  }, [query, customItems]);

  function renderItem(item: PickerItem) {
    return (
      <button
        key={item.key}
        type="button"
        className={styles.item}
        title={item.label}
        onClick={(e) => {
          e.stopPropagation();
          onPick(item.content);
        }}
      >
        {item.imageUrl ? (
          <img className={styles.itemImg} src={item.imageUrl} alt={item.label} loading="lazy" />
        ) : (
          item.content
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

      <div className={styles.body}>
        {loading ? (
          <p className={styles.message}>{t("common:loading")}</p>
        ) : query.trim() ? (
          searchResults && searchResults.length > 0 ? (
            <div className={styles.grid}>{searchResults.map(renderItem)}</div>
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
            <div className={styles.grid}>{customItems.map(renderItem)}</div>
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

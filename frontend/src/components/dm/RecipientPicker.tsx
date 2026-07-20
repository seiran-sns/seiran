import { KeyboardEvent, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, ActorSuggestion } from "../../api/client";
import styles from "./RecipientPicker.module.css";

export interface RecipientChip {
  actorId: string;
  username: string;
  domain: string;
  displayName?: string;
  actorType: string;
  avatarUrl?: string;
  /** サジェスト非選択で確定した文字列が既知のアクターと解決できなかった場合。 */
  unresolved?: boolean;
}

interface RecipientPickerProps {
  value: RecipientChip[];
  onChange: (chips: RecipientChip[]) => void;
}

const CONFIRM_CHARS = [",", "\n", " ", "\t"];

/** 確定済みchip一覧にBskyアクターとそれ以外（Fedi/local）が混在するかを判定する
 * （Bsky DMは1対1のみのため、Bsky宛先1人に他の宛先が混ざるのを禁止する）。 */
function hasProtocolConflict(chips: RecipientChip[]): boolean {
  const hasBsky = chips.some((c) => c.actorType === "bsky");
  const hasOther = chips.some((c) => c.actorType !== "bsky");
  return hasBsky && hasOther;
}

export default function RecipientPicker({ value, onChange }: RecipientPickerProps) {
  const { t } = useTranslation();
  const [input, setInput] = useState("");
  const [suggestions, setSuggestions] = useState<ActorSuggestion[]>([]);
  const [showSuggestions, setShowSuggestions] = useState(false);
  const [warning, setWarning] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const q = input.trim();
    if (q.length === 0) {
      setSuggestions([]);
      return;
    }
    let cancelled = false;
    const controller = new AbortController();
    const timer = window.setTimeout(() => {
      api.actors
        .search(q, 8, controller.signal)
        .then((rows) => !cancelled && setSuggestions(rows))
        .catch(() => {});
    }, 300);
    return () => {
      cancelled = true;
      controller.abort();
      window.clearTimeout(timer);
    };
  }, [input]);

  function addChip(chip: RecipientChip) {
    if (value.some((c) => c.actorId === chip.actorId)) {
      setInput("");
      setSuggestions([]);
      setShowSuggestions(false);
      return;
    }
    const next = [...value, chip];
    if (hasProtocolConflict(next)) {
      setWarning(t("dm:recipientPicker.protocolConflictWarning"));
    } else {
      setWarning("");
    }
    onChange(next);
    setInput("");
    setSuggestions([]);
    setShowSuggestions(false);
  }

  function selectSuggestion(s: ActorSuggestion) {
    addChip({
      actorId: s.actor_id,
      username: s.username,
      domain: s.domain,
      displayName: s.display_name,
      actorType: s.actor_type,
      avatarUrl: s.avatar_url,
    });
    inputRef.current?.focus();
  }

  async function confirmTyped() {
    const q = input.trim();
    if (!q) return;
    try {
      const rows = await api.actors.search(q, 1);
      const exact = rows[0];
      if (exact) {
        addChip({
          actorId: exact.actor_id,
          username: exact.username,
          domain: exact.domain,
          displayName: exact.display_name,
          actorType: exact.actor_type,
          avatarUrl: exact.avatar_url,
        });
      } else {
        // 解決できない入力はそのままchip化し、赤字表示で警告する（送信時にバックエンドが拒否する）。
        addChip({ actorId: q, username: q, domain: "", actorType: "unknown", unresolved: true });
        setWarning(t("dm:recipientPicker.unresolvedWarning", { input: q }));
      }
    } catch {
      addChip({ actorId: q, username: q, domain: "", actorType: "unknown", unresolved: true });
    }
  }

  function handleKeyDown(e: KeyboardEvent<HTMLInputElement>) {
    if (CONFIRM_CHARS.includes(e.key) || (e.key === "Tab" && input.trim())) {
      if (input.trim()) {
        e.preventDefault();
        confirmTyped();
      }
    } else if (e.key === "Backspace" && input.length === 0 && value.length > 0) {
      removeChip(value[value.length - 1].actorId);
    }
  }

  function removeChip(actorId: string) {
    const next = value.filter((c) => c.actorId !== actorId);
    onChange(next);
    setWarning(hasProtocolConflict(next) ? t("dm:recipientPicker.protocolConflictWarning") : "");
  }

  const conflict = hasProtocolConflict(value);

  return (
    <div className={styles.wrap}>
      <div className={styles.chipInputRow} onClick={() => inputRef.current?.focus()}>
        {value.map((chip, idx) => {
          // 混在エラー時、Bskyでない側の最後に追加された1つ（または2番目以降）を赤くする単純化として、
          // 「多数派プロトコルと異なるchip」を赤字にする。
          const bskyCount = value.filter((c) => c.actorType === "bsky").length;
          const majorityIsBsky = bskyCount * 2 > value.length;
          const isMinorityConflict = conflict && (majorityIsBsky ? chip.actorType !== "bsky" : chip.actorType === "bsky");
          const isInvalid = chip.unresolved || isMinorityConflict;
          return (
            <span key={`${chip.actorId}-${idx}`} className={`${styles.chip} ${isInvalid ? styles.chipInvalid : ""}`}>
              {chip.avatarUrl && <img src={chip.avatarUrl} alt="" className={styles.chipAvatar} />}
              <span className={styles.chipLabel}>
                {chip.displayName || chip.username}
                {chip.domain ? `@${chip.domain}` : ""}
              </span>
              <button type="button" className={styles.chipRemove} onClick={() => removeChip(chip.actorId)}>
                ×
              </button>
            </span>
          );
        })}
        <input
          ref={inputRef}
          className={styles.input}
          value={input}
          onChange={(e) => {
            setInput(e.target.value);
            setShowSuggestions(true);
          }}
          onFocus={() => setShowSuggestions(true)}
          onBlur={() => confirmTyped()}
          onKeyDown={handleKeyDown}
          placeholder={value.length === 0 ? t("dm:recipientPicker.placeholder") : ""}
          autoComplete="off"
        />
      </div>
      {showSuggestions && suggestions.length > 0 && (
        <ul className={styles.suggestList}>
          {suggestions.map((s) => (
            <li key={s.actor_id}>
              <button
                type="button"
                className={styles.suggestItem}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => selectSuggestion(s)}
              >
                <span className={styles.suggestAvatar}>
                  {s.avatar_url ? <img src={s.avatar_url} alt="" /> : <span>{(s.display_name || s.username)[0]?.toUpperCase()}</span>}
                </span>
                <span className={styles.suggestName}>
                  {s.display_name || s.username}
                  <span className={styles.suggestHandle}>
                    @{s.username}
                    {s.domain ? `@${s.domain}` : ""}
                  </span>
                </span>
                <span className={styles.suggestType}>{s.actor_type}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {warning && <div className={styles.warningBubble}>{warning}</div>}
    </div>
  );
}

import { FormEvent, ReactNode, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ActorSuggestion } from "../../api/client";
import DisplayName from "../common/DisplayName";
import UserIdPickerPanel from "../note/UserIdPickerPanel";
import styles from "./SearchConditionsField.module.css";

/** 条件に指定したユーザー（表示に要る分だけ持つ）。 */
export interface SearchConditionUser {
  actorId: string;
  /** `@user@host` 形式のハンドル（URL に載せ、読み込み時にここから引き直す）。 */
  handle: string;
  displayName?: string;
  emojis?: Record<string, string>;
}

/** 投稿検索の条件。どれも1つずつで、指定したものは AND で効く
 * （Misskey・Mastodon の互換 API と Bluesky の検索がどれも1つずつしか指定できないため）。 */
export interface SearchConditions {
  /** この日以降（`YYYY-MM-DD`、閲覧者のタイムゾーンの日付）。 */
  since?: string;
  /** この日以前（`YYYY-MM-DD`、その日を含む）。 */
  until?: string;
  user?: SearchConditionUser;
  /** `local`・`fediverse`・`bluesky`・ホスト名。 */
  server?: string;
  /** ハッシュタグ（先頭 `#` なし）。 */
  tag?: string;
}

type ConditionKind = "since" | "until" | "user" | "server" | "tag";
const KINDS: ConditionKind[] = ["since", "until", "user", "server", "tag"];
const SERVER_PRESETS = ["local", "fediverse", "bluesky"] as const;

export function hasConditions(c: SearchConditions): boolean {
  return Boolean(c.since || c.until || c.user || c.server || c.tag);
}

/** サーバー指定の入力をホスト名だけにする（`https://`・パス・先頭の `@` を除き、小文字にする）。 */
export function normalizeServerInput(value: string): string {
  return value
    .trim()
    .replace(/^https?:\/\//i, "")
    .replace(/^@/, "")
    .replace(/\/.*$/, "")
    .toLowerCase();
}

/** ハッシュタグの入力を先頭 `#` なしにする。 */
export function normalizeTagInput(value: string): string {
  return value.trim().replace(/^#+/, "");
}

interface Props {
  value: SearchConditions;
  onChange: (value: SearchConditions) => void;
}

/** 投稿検索の条件欄。設定した条件をタグのように並べ、「条件を追加」から1つずつ足す。 */
export default function SearchConditionsField({ value, onChange }: Props) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (e: PointerEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  const remove = (kind: ConditionKind) => onChange({ ...value, [kind]: undefined });
  const add = (patch: SearchConditions) => {
    onChange({ ...value, ...patch });
    setOpen(false);
  };

  const serverLabel = (server: string) =>
    (SERVER_PRESETS as readonly string[]).includes(server)
      ? t(`search:searchPage.conditions.server.${server}`)
      : server;

  const chips: { kind: ConditionKind; label: ReactNode }[] = [];
  if (value.since) chips.push({ kind: "since", label: t("search:searchPage.conditions.labels.since", { date: value.since }) });
  if (value.until) chips.push({ kind: "until", label: t("search:searchPage.conditions.labels.until", { date: value.until }) });
  if (value.user)
    chips.push({
      kind: "user",
      label: (
        <>
          {t("search:searchPage.conditions.labels.user")}
          <DisplayName name={value.user.displayName} fallback={value.user.handle} emojis={value.user.emojis} />
        </>
      ),
    });
  if (value.server)
    chips.push({ kind: "server", label: t("search:searchPage.conditions.labels.server", { server: serverLabel(value.server) }) });
  if (value.tag) chips.push({ kind: "tag", label: `#${value.tag}` });

  return (
    <div className={styles.field} ref={wrapRef}>
      <div className={styles.box} role="group" aria-label={t("search:searchPage.conditions.label")}>
        {chips.map((chip) => (
          <span key={chip.kind} className={styles.chip} data-kind={chip.kind}>
            <span className={styles.chipLabel}>{chip.label}</span>
            <button
              type="button"
              className={styles.chipRemove}
              aria-label={t("search:searchPage.conditions.remove")}
              onClick={() => remove(chip.kind)}
            >
              ✕
            </button>
          </span>
        ))}
        <button
          type="button"
          className={styles.addBtn}
          aria-expanded={open}
          onClick={() => setOpen((v) => !v)}
        >
          + {t("search:searchPage.conditions.add")}
        </button>
      </div>
      {open && <ConditionBuilder value={value} onAdd={add} />}
    </div>
  );
}

interface BuilderProps {
  value: SearchConditions;
  onAdd: (patch: SearchConditions) => void;
}

/** 条件を1つ作って足す箱。種類を選び、種類ごとの入力欄で値を決める。 */
function ConditionBuilder({ value, onAdd }: BuilderProps) {
  const { t } = useTranslation();
  const [kind, setKind] = useState<ConditionKind>("since");
  const [date, setDate] = useState("");
  const [serverChoice, setServerChoice] = useState<string>("local");
  const [host, setHost] = useState("");
  const [tag, setTag] = useState("");

  const pickUser = (actor: ActorSuggestion) =>
    onAdd({
      user: {
        actorId: actor.actor_id,
        handle: actor.target,
        displayName: actor.display_name,
        emojis: actor.emojis,
      },
    });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (kind === "since" || kind === "until") {
      if (date) onAdd({ [kind]: date });
    } else if (kind === "server") {
      const server = serverChoice === "host" ? normalizeServerInput(host) : serverChoice;
      if (server) onAdd({ server });
    } else if (kind === "tag") {
      const normalized = normalizeTagInput(tag);
      if (normalized) onAdd({ tag: normalized });
    }
  };

  const valid =
    kind === "since" || kind === "until"
      ? Boolean(date)
      : kind === "server"
        ? serverChoice !== "host" || Boolean(normalizeServerInput(host))
        : kind === "tag"
          ? Boolean(normalizeTagInput(tag))
          : false;

  return (
    <div className={styles.popover}>
      <div className={styles.kinds} role="radiogroup" aria-label={t("search:searchPage.conditions.kindLabel")}>
        {KINDS.map((k) => (
          <button
            key={k}
            type="button"
            role="radio"
            aria-checked={kind === k}
            className={kind === k ? `${styles.kind} ${styles.kindActive}` : styles.kind}
            onClick={() => setKind(k)}
          >
            {t(`search:searchPage.conditions.kinds.${k}`)}
          </button>
        ))}
      </div>
      {value[kind] !== undefined && <p className={styles.note}>{t("search:searchPage.conditions.replaceNote")}</p>}
      {kind === "user" ? (
        <UserIdPickerPanel onPick={pickUser} />
      ) : (
        <form className={styles.form} onSubmit={submit}>
          {(kind === "since" || kind === "until") && (
            <input
              type="date"
              className={styles.input}
              value={date}
              max={kind === "since" ? value.until : undefined}
              min={kind === "until" ? value.since : undefined}
              onChange={(e) => setDate(e.target.value)}
              aria-label={t(`search:searchPage.conditions.kinds.${kind}`)}
              autoFocus
            />
          )}
          {kind === "server" && (
            <div className={styles.serverChoices}>
              {[...SERVER_PRESETS, "host"].map((choice) => (
                <label key={choice} className={styles.radio}>
                  <input
                    type="radio"
                    name="search-server"
                    value={choice}
                    checked={serverChoice === choice}
                    onChange={() => setServerChoice(choice)}
                  />
                  {t(`search:searchPage.conditions.server.${choice}`)}
                </label>
              ))}
              {serverChoice === "host" && (
                <input
                  type="text"
                  className={styles.input}
                  value={host}
                  placeholder={t("search:searchPage.conditions.server.hostPlaceholder")}
                  onChange={(e) => setHost(e.target.value)}
                  autoComplete="off"
                  autoFocus
                />
              )}
            </div>
          )}
          {kind === "tag" && (
            <input
              type="text"
              className={styles.input}
              value={tag}
              placeholder={t("search:searchPage.conditions.tagPlaceholder")}
              onChange={(e) => setTag(e.target.value)}
              autoComplete="off"
              autoFocus
            />
          )}
          <button type="submit" className={styles.applyBtn} disabled={!valid}>
            {t("search:searchPage.conditions.apply")}
          </button>
        </form>
      )}
    </div>
  );
}

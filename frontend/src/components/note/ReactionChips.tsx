import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ReactionActor, ReactionSummary } from "../../api/client";
import { fitChipsInLines } from "../../lib/chipRows";
import { fetchReactionActors } from "../../lib/repostReactions";
import { fetchCustomEmojiShortcodes, parseCustomEmojiShortcode } from "../../lib/customEmojis";
import RemoteImg from "../common/RemoteImg";

import UserItem from "../common/UserItem";
import EmojiContextMenu from "./EmojiContextMenu";
import TwemojiEmoji from "../common/TwemojiEmoji";
import styles from "./ReactionChips.module.css";

interface ReactionChipsProps {
  noteId: string;
  /** リポストラッパー越しに表示しているときのラッパーID。`reactions` にラッパー宛ての分が
   * 合算されているので、リアクションした人一覧もラッパー側から取得して合わせる。 */
  viaRepostId?: string;
  reactions?: ReactionSummary[];
  /** チップクリック時に同じ絵文字でトグル（追加/取消・切替）する（未指定なら非インタラクティブ）。 */
  onToggle?: (emoji: string) => void;
  /** リアクション操作中は true。全チップのクリックを無効化する（1投稿1リアクションまでのため）。 */
  disabled?: boolean;
  /** 本文・添付等と同じ50pxインデントを付けるか（`LinkCard`の`indent`と同じ役割）。
   * 通常のNoteCardではtrue（アバター分のインデント）、ポスト詳細の拡大表示（large）や
   * 引用カード内など、呼び出し側で既にインデントを解消済みの文脈ではfalseを渡す。 */
  indent?: boolean;
}

/** たたんでいる間に見せる行数。 */
const COLLAPSED_LINES = 2;

/** 届いたリアクションの集計チップ表示。クリックで同じ絵文字を自分も付ける/取り消す/切り替える。
 * ホバーするとそのリアクションを付けたアクター一覧（アイコン付き）をポップオーバー表示する。
 * カスタム絵文字（`:shortcode:`）はこのサーバーの `custom_emojis` に登録済みのものしか送信できない
 * （Fedi/Bsky受信のよそのサーバー由来のカスタム絵文字リアクションはクリックで追いリアクションできず、
 * 自分が既に付けている分の取消のみ許可する）。
 *
 * `COLLAPSED_LINES` 行に収まらないときはたたみ、最後の行の末尾に「全部」ボタンを出す。広げたら
 * 末尾に「たたむ」ボタンを出す。何個見せるかは実測した幅から決める（`lib/chipRows.ts`）。
 * 見せないチップも幅を測るために描画したまま、流れの外へ出して隠す。 */
export default function ReactionChips({
  noteId,
  viaRepostId,
  reactions,
  onToggle,
  disabled,
  indent = true,
}: ReactionChipsProps) {
  const { t } = useTranslation();
  const [knownShortcodes, setKnownShortcodes] = useState<Set<string> | null>(null);
  const [expanded, setExpanded] = useState(false);
  // 実測前（null）は全部見せる。
  const [fit, setFit] = useState<{ visible: number; overflow: boolean } | null>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const measureButtonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    fetchCustomEmojiShortcodes().then(setKnownShortcodes);
  }, []);

  // 欄の幅・チップの幅（絵文字画像の読み込みで変わる）が変わるたびに測り直す。
  useLayoutEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const chips = Array.from(wrap.querySelectorAll<HTMLElement>(":scope > [data-reaction-chip]"));
    const measure = () => {
      const next = fitChipsInLines(
        chips.map((el) => el.getBoundingClientRect().width),
        wrap.clientWidth,
        parseFloat(getComputedStyle(wrap).columnGap) || 0,
        measureButtonRef.current?.getBoundingClientRect().width ?? 0,
        COLLAPSED_LINES
      );
      setFit((prev) => (prev && prev.visible === next.visible && prev.overflow === next.overflow ? prev : next));
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(wrap);
    chips.forEach((el) => observer.observe(el));
    return () => observer.disconnect();
  }, [reactions]);

  if (!reactions || reactions.length === 0) return null;
  const overflow = fit?.overflow ?? false;
  const visible = overflow && !expanded ? (fit?.visible ?? reactions.length) : reactions.length;
  const toggle = (label: string) => (
    <button
      type="button"
      className={styles.toggle}
      onClick={(e) => {
        e.stopPropagation();
        setExpanded((v) => !v);
      }}
    >
      {label}
    </button>
  );
  return (
    <div ref={wrapRef} className={`${styles.wrap} ${indent ? styles.indent : ""}`}>
      {reactions.map((r, i) => (
        <ReactionChip
          key={r.emoji}
          hidden={i >= visible}
          noteId={noteId}
          viaRepostId={viaRepostId}
          reaction={r}
          onToggle={onToggle}
          disabled={disabled}
          knownShortcodes={knownShortcodes}
        />
      ))}
      {overflow && (expanded ? toggle(t("home:reactionChips.collapse")) : toggle(t("home:reactionChips.showAll")))}
      {/* 「全部」ボタンの幅を測るための見えない複製。 */}
      <button ref={measureButtonRef} type="button" className={`${styles.toggle} ${styles.offFlow}`} aria-hidden tabIndex={-1}>
        {t("home:reactionChips.showAll")}
      </button>
    </div>
  );
}

interface ReactionChipProps {
  noteId: string;
  viaRepostId?: string;
  reaction: ReactionSummary;
  onToggle?: (emoji: string) => void;
  disabled?: boolean;
  /** このサーバーに登録済みのカスタム絵文字 shortcode 一覧。未ロードなら null（ロード完了まで安全側でブロック）。 */
  knownShortcodes: Set<string> | null;
  /** たたんでいて見せないチップ。幅を測るため描画はしたまま流れの外へ出す。 */
  hidden?: boolean;
}

function ReactionChip({ noteId, viaRepostId, reaction: r, onToggle, disabled, knownShortcodes, hidden }: ReactionChipProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [actors, setActors] = useState<ReactionActor[] | null>(null);
  const [failed, setFailed] = useState(false);
  const fetchedRef = useRef(false);
  const chipRef = useRef<HTMLButtonElement>(null);

  // 自分が既に付けているリアクションの取消は常に許可する（バックエンドは絵文字の実在確認をせず削除する）。
  // 新規追加は、カスタム絵文字ならこのサーバーに登録済みの場合のみ許可する
  // （未登録のカスタム絵文字を送信すると `create_reaction` が `UNKNOWN_EMOJI` で拒否するため）。
  const shortcode = parseCustomEmojiShortcode(r.emoji);
  const addBlocked = !r.reactedByMe && shortcode !== null && !(knownShortcodes?.has(shortcode) ?? false);

  const onEnter = useCallback(() => {
    if (!fetchedRef.current) {
      fetchedRef.current = true;
      setLoading(true);
      fetchReactionActors(noteId, viaRepostId, r.emoji)
        .then(setActors)
        .catch(() => setFailed(true))
        .finally(() => setLoading(false));
    }
    setOpen(true);
  }, [noteId, viaRepostId, r.emoji]);

  const onLeave = useCallback(() => setOpen(false), []);

  // ポップオーバーはカーソルがチップ（ボタン）の上にある間だけ出す。ポップオーバー自体は
  // `pointer-events: none` で、下に隠れたリポスト・リアクション等のボタンをそのまま押せる。
  //
  // `onMouseEnter`/`onMouseLeave`（Reactの合成イベント）はDOM階層ではなくReactツリー上の
  // 親子関係に基づいて伝播範囲が決まる。EmojiContextMenu配下のメニュー・インポートダイアログは
  // `createPortal`でDOM上はbody直下に描画されるが、Reactツリー上はこのチップの子孫のまま
  // なので合成イベントの mouseleave が発火せず、マウスをそちらへ移動してもポップオーバーが
  // 開いたままになってしまう。DOM階層（`relatedTarget`が実際にこの要素の外かどうか）で正しく
  // 判定するため、ネイティブイベントリスナーで実装する。
  useEffect(() => {
    const el = chipRef.current;
    if (!el) return;
    function handleOver(e: MouseEvent) {
      const related = e.relatedTarget as Node | null;
      if (related && chipRef.current?.contains(related)) return;
      onEnter();
    }
    function handleOut(e: MouseEvent) {
      const related = e.relatedTarget as Node | null;
      if (related && chipRef.current?.contains(related)) return;
      onLeave();
    }
    el.addEventListener("mouseover", handleOver);
    el.addEventListener("mouseout", handleOut);
    return () => {
      el.removeEventListener("mouseover", handleOver);
      el.removeEventListener("mouseout", handleOut);
    };
  }, [onEnter, onLeave]);

  return (
    <div className={`${styles.chipWrap} ${hidden ? styles.offFlow : ""}`} data-reaction-chip aria-hidden={hidden || undefined}>
      <button
        type="button"
        ref={chipRef}
        className={`${styles.chip} ${r.reactedByMe ? styles.chipActive : ""}`}
        onClick={(e) => {
          e.stopPropagation();
          if (!onToggle || disabled || addBlocked) return;
          onToggle(r.emoji);
        }}
      >
        {r.emojiUrl && shortcode ? (
          <EmojiContextMenu shortcode={shortcode} imageUrl={r.emojiUrl}>
            <RemoteImg className={styles.emojiImg} src={r.emojiUrl} alt={r.emoji} loading="lazy" data-emoji="" />
          </EmojiContextMenu>
        ) : r.emojiUrl ? (
          <RemoteImg className={styles.emojiImg} src={r.emojiUrl} alt={r.emoji} loading="lazy" data-emoji="" />
        ) : (
          <TwemojiEmoji emoji={r.emoji} className={styles.emojiImg} />
        )}
        <span className={styles.count}>{r.count}</span>
      </button>

      {open && (
        <div className={styles.popover}>
          {shortcode && <p className={styles.emojiName}>{r.emoji}</p>}
          {loading && <p className={styles.dim}>{t("common:loading")}</p>}
          {failed && <p className={styles.dim}>{t("home:reactionChips.actorsFetchFailed")}</p>}
          {actors && actors.length > 0 && (
            <ul className={styles.actorList}>
              {actors.map((a) => (
                <li key={a.id} className={styles.actorRow}>
                  <UserItem avatarUrl={a.avatarUrl} username={a.username} displayName={a.displayName} emojis={a.emojis} avatarSize={22} />
                </li>
              ))}
            </ul>
          )}
          <p className={styles.hint}>
            {r.reactedByMe
              ? t("home:reactionChips.clickToRemove")
              : addBlocked
                ? t("home:reactionChips.customEmojiBlocked")
                : t("home:reactionChips.clickToAdd")}
          </p>
        </div>
      )}
    </div>
  );
}

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, ReactionActor, ReactionSummary } from "../../api/client";
import { fetchCustomEmojiShortcodes, parseCustomEmojiShortcode } from "../../lib/customEmojis";
import { mediaUrl } from "../../utils/mediaProxy";
import Avatar from "./Avatar";
import EmojiContextMenu from "./EmojiContextMenu";
import TwemojiEmoji from "../common/TwemojiEmoji";
import styles from "./ReactionChips.module.css";

interface ReactionChipsProps {
  noteId: string;
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

/** 届いたリアクションの集計チップ表示。クリックで同じ絵文字を自分も付ける/取り消す/切り替える。
 * ホバーするとそのリアクションを付けたアクター一覧（アイコン付き）をポップオーバー表示する。
 * カスタム絵文字（`:shortcode:`）はこのサーバーの `custom_emojis` に登録済みのものしか送信できない
 * （Fedi/Bsky受信のよそのサーバー由来のカスタム絵文字リアクションはクリックで追いリアクションできず、
 * 自分が既に付けている分の取消のみ許可する）。 */
export default function ReactionChips({
  noteId,
  reactions,
  onToggle,
  disabled,
  indent = true,
}: ReactionChipsProps) {
  const [knownShortcodes, setKnownShortcodes] = useState<Set<string> | null>(null);

  useEffect(() => {
    fetchCustomEmojiShortcodes().then(setKnownShortcodes);
  }, []);

  if (!reactions || reactions.length === 0) return null;
  return (
    <div className={`${styles.wrap} ${indent ? styles.indent : ""}`}>
      {reactions.map((r) => (
        <ReactionChip
          key={r.emoji}
          noteId={noteId}
          reaction={r}
          onToggle={onToggle}
          disabled={disabled}
          knownShortcodes={knownShortcodes}
        />
      ))}
    </div>
  );
}

interface ReactionChipProps {
  noteId: string;
  reaction: ReactionSummary;
  onToggle?: (emoji: string) => void;
  disabled?: boolean;
  /** このサーバーに登録済みのカスタム絵文字 shortcode 一覧。未ロードなら null（ロード完了まで安全側でブロック）。 */
  knownShortcodes: Set<string> | null;
}

function ReactionChip({ noteId, reaction: r, onToggle, disabled, knownShortcodes }: ReactionChipProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [actors, setActors] = useState<ReactionActor[] | null>(null);
  const [failed, setFailed] = useState(false);
  const fetchedRef = useRef(false);
  const timerRef = useRef<number | null>(null);
  const wrapRef = useRef<HTMLDivElement>(null);

  // 自分が既に付けているリアクションの取消は常に許可する（バックエンドは絵文字の実在確認をせず削除する）。
  // 新規追加は、カスタム絵文字ならこのサーバーに登録済みの場合のみ許可する
  // （未登録のカスタム絵文字を送信すると `create_reaction` が `UNKNOWN_EMOJI` で拒否するため）。
  const shortcode = parseCustomEmojiShortcode(r.emoji);
  const addBlocked = !r.reactedByMe && shortcode !== null && !(knownShortcodes?.has(shortcode) ?? false);

  const onEnter = useCallback(() => {
    if (!fetchedRef.current) {
      fetchedRef.current = true;
      setLoading(true);
      api.notes
        .reactionActors(noteId, r.emoji)
        .then((res) => setActors(res.actors))
        .catch(() => setFailed(true))
        .finally(() => setLoading(false));
    }
    if (timerRef.current) window.clearTimeout(timerRef.current);
    setOpen(true);
  }, [noteId, r.emoji]);

  const onLeave = useCallback(() => {
    // 少し遅延させてから閉じる（ポップオーバーへのカーソル移動を許容）。
    timerRef.current = window.setTimeout(() => setOpen(false), 120);
  }, []);

  // `onMouseEnter`/`onMouseLeave`（Reactの合成イベント）はDOM階層ではなくReactツリー上の
  // 親子関係に基づいて伝播範囲が決まる。EmojiContextMenu配下のメニュー・インポートダイアログは
  // `createPortal`でDOM上はbody直下に描画されるが、Reactツリー上はこの chipWrap の子孫のまま
  // なので合成イベントの mouseleave が発火せず、マウスをそちらへ移動してもポップオーバーが
  // 開いたままになってしまう。DOM階層（`relatedTarget`が実際にこの要素の外かどうか）で正しく
  // 判定するため、ネイティブイベントリスナーで実装する。
  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    function handleOver(e: MouseEvent) {
      const related = e.relatedTarget as Node | null;
      if (related && wrapRef.current?.contains(related)) return;
      onEnter();
    }
    function handleOut(e: MouseEvent) {
      const related = e.relatedTarget as Node | null;
      if (related && wrapRef.current?.contains(related)) return;
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
    <div className={styles.chipWrap} ref={wrapRef}>
      <button
        type="button"
        className={`${styles.chip} ${r.reactedByMe ? styles.chipActive : ""}`}
        onClick={(e) => {
          e.stopPropagation();
          if (!onToggle || disabled || addBlocked) return;
          onToggle(r.emoji);
        }}
      >
        {r.emojiUrl && shortcode ? (
          <EmojiContextMenu shortcode={shortcode} imageUrl={r.emojiUrl}>
            <img className={styles.emojiImg} src={mediaUrl(r.emojiUrl)} alt={r.emoji} loading="lazy" />
          </EmojiContextMenu>
        ) : r.emojiUrl ? (
          <img className={styles.emojiImg} src={mediaUrl(r.emojiUrl)} alt={r.emoji} loading="lazy" />
        ) : (
          <TwemojiEmoji emoji={r.emoji} className={styles.emojiImg} />
        )}
        <span className={styles.count}>{r.count}</span>
      </button>

      {open && (
        <div className={styles.popover} onClick={(e) => e.stopPropagation()}>
          {loading && <p className={styles.dim}>{t("common:loading")}</p>}
          {failed && <p className={styles.dim}>{t("home:reactionChips.actorsFetchFailed")}</p>}
          {actors && actors.length > 0 && (
            <ul className={styles.actorList}>
              {actors.map((a) => (
                <li key={a.id} className={styles.actorRow}>
                  <Avatar url={a.avatarUrl} name={a.displayName || a.username} size={22} />
                  <span className={styles.actorName}>{a.displayName || a.username}</span>
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

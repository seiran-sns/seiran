import { ReactNode, useRef, useState, type CSSProperties } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { api, Note } from "../../api/client";
import Avatar from "./Avatar";
import EmojiText from "./EmojiText";
import ContentWarningText from "./ContentWarningText";
import TwemojiEmoji from "../common/TwemojiEmoji";
import { acct } from "../../lib/format";
import styles from "./NoteHoverPreview.module.css";
import DisplayName from "../common/DisplayName";

interface NoteHoverPreviewProps {
  noteId: string;
  children: ReactNode;
  className?: string;
  /** ポップアップの表示位置。"bottom"（既定）は子要素の直下、"left"は子要素の左側。
   * "left"は右ペインの通知アイテムのように、真下に出すとポップアップへマウスが乗って
   * 消えなくなり下のアイテムが押せなくなる場面で使う。 */
  side?: "bottom" | "left";
}

/**
 * 子要素へのマウスオーバー中、指定されたポストの概要を表示する。
 * 返信先インジケータと通知アイテムで共用する。
 */
export default function NoteHoverPreview({ noteId, children, className, side = "bottom" }: NoteHoverPreviewProps) {
  const { t } = useTranslation();
  const [target, setTarget] = useState<Note | null>(null);
  const [loading, setLoading] = useState(false);
  const [failed, setFailed] = useState(false);
  // 一度でもマウスが乗ったらポップアップをDOMに置く。表示・非表示はCSSの`:hover`が
  // 決める（.module.cssの`.wrap:hover > .popup`）。mouseenter/mouseleaveの対で開閉
  // ステートを持つと、重ねた通知の広げ畳みのようにカーソルを動かさずに要素が
  // 動く場面や、ホバー中に子要素が差し替わる場面でleaveを取りこぼしたとき、
  // 閉じる契機が二度と来ずポップアップが残り続ける。`:hover`はブラウザが
  // 現在のカーソル位置から都度判定するため、この取りこぼしが起きない。
  const [activated, setActivated] = useState(false);
  // クリック直後の誤表示を抑える（onClickCaptureのコメント参照）。次にマウスが
  // 離れるまで続く。leaveを取りこぼしても「出ないまま」に倒れるだけで残り続けはしない。
  const [suppressed, setSuppressed] = useState(false);
  const [showContent, setShowContent] = useState(false);
  const [fixedStyle, setFixedStyle] = useState<CSSProperties | null>(null);
  const [effectiveSide, setEffectiveSide] = useState<"bottom" | "left" | "right">(side);
  const fetchedRef = useRef(false);
  const wrapRef = useRef<HTMLSpanElement>(null);

  function ensureFetched() {
    if (fetchedRef.current) return;
    fetchedRef.current = true;
    setLoading(true);
    api.notes
      .get(noteId)
      .then(setTarget)
      .catch(() => setFailed(true))
      .finally(() => setLoading(false));
  }

  function onEnter() {
    ensureFetched();
    // side="left"は右ペインの通知アイテムで使う想定。右ペインは独自に縦スクロールする
    // コンテナ（AppShellの.rightScroll）であり、CSSの仕様上「縦だけauto、横はvisible」
    // という指定はできない（片方がvisibleでない場合は両方autoに揃えられる）ため、
    // CSSのみの絶対配置だとポップアップの左側がそのスクロール境界でクリップされて
    // ほぼ見えなくなる。position: fixedへ切り替え、
    // トリガー要素の実測座標を使って画面基準で配置することでこれを回避する。
    if (side === "left" && wrapRef.current) {
      const rect = wrapRef.current.getBoundingClientRect();
      const POPUP_WIDTH = 320;
      const GAP = 10;
      const VIEWPORT_MARGIN = 8;
      const REQUIRED = GAP + POPUP_WIDTH + VIEWPORT_MARGIN;
      // 通知アイテムの一覧は右ペイン（home画面の【クイック通知】。左側に大きく
      // 余白がある）だけでなく中央ペイン（/notifications画面。アイテム自体が
      // 画面左寄りにあり、左側の余白はナビ分程度しかない）でも使われる。左右
      // どちらに実際の余白があるかを実測し、余白のある側へ出す。スマホ幅など
      // どちらにも余白が無い場合のみアイテム下へフォールバックする（左固定だと
      // 中央ペイン側で画面外へはみ出し、常に下に出すと他アイテムに重なる）。
      if (rect.left >= REQUIRED) {
        setEffectiveSide("left");
        setFixedStyle({
          position: "fixed",
          top: rect.top + rect.height / 2,
          right: window.innerWidth - rect.left + GAP,
          left: "auto",
          transform: "translateY(-50%)",
        });
      } else if (window.innerWidth - rect.right >= REQUIRED) {
        setEffectiveSide("right");
        setFixedStyle({
          position: "fixed",
          top: rect.top + rect.height / 2,
          left: rect.right + GAP,
          right: "auto",
          transform: "translateY(-50%)",
        });
      } else {
        setEffectiveSide("bottom");
        setFixedStyle(null);
      }
    }
    setActivated(true);
  }

  function onLeave() {
    setSuppressed(false);
  }

  // 子要素内の対ユーザー操作メニュー（`UserContextMenu`/`UserLinkTag`）は
  // `document.body`直下へポータルされ、この`wrap`要素の真上に重なって表示される。
  // メニュー項目クリックでそのポータルがDOMから消えると、同じ座標に残っていた
  // この`wrap`要素へ「新規にマウスが入った」というmouseenterがブラウザから
  // 発火し、カーソルがこの要素の上にあるため`:hover`も成り立ってポップアップが出てしまう。
  // クリック自体はReactツリーに沿ってこの要素までバブルするため、ここで次にマウスが
  // 離れるまで表示を抑える（ポップアップ内のリンクを押して画面遷移した場合も同様）。
  function onClickCapture() {
    setSuppressed(true);
  }

  const wrapClass = [styles.wrap, suppressed ? styles.suppressed : null, className]
    .filter(Boolean)
    .join(" ");

  return (
    <span
      ref={wrapRef}
      className={wrapClass}
      onMouseEnter={onEnter}
      onMouseLeave={onLeave}
      onClickCapture={onClickCapture}
    >
      {children}
      {activated && (
        <span
          className={
            effectiveSide === "left"
              ? `${styles.popup} ${styles.popupLeft}`
              : effectiveSide === "right"
                ? `${styles.popup} ${styles.popupRight}`
                : styles.popup
          }
          style={effectiveSide !== "bottom" && fixedStyle ? fixedStyle : undefined}
        >
          {loading && <span className={styles.dim}>{t("common:loading")}</span>}
          {failed && <span className={styles.dim}>{t("home:replyIndicator.fetchFailed")}</span>}
          {target && (
            <Link to={`/notes/${target.id}`} className={styles.card} onClick={(e) => e.stopPropagation()}>
              <span className={styles.head}>
                <Avatar
                  url={target.user.avatarUrl}
                  name={target.user.displayName || target.user.username}
                  size={26}
                />
                <span className={styles.names}>
                  <span className={styles.name}>
                  <DisplayName name={target.user.displayName} fallback={target.user.username} emojis={target.emojis} />
                </span>
                  <span className={styles.acctText}>{acct(target)}</span>
                </span>
              </span>
              {target.contentWarning && (
                <span className={styles.cw}>
                  <span className={styles.cwText}>
                    <TwemojiEmoji emoji="⚠️" /> <ContentWarningText note={target} />
                  </span>
                  <button
                    type="button"
                    onClick={(e) => {
                      e.preventDefault();
                      e.stopPropagation();
                      setShowContent((shown) => !shown);
                    }}
                  >
                    {showContent
                      ? t("home:noteCard.hideContent")
                      : t("home:noteCard.showContent")}
                  </button>
                </span>
              )}
              {(!target.contentWarning || showContent) && (
                <span className={styles.text}>
                  <EmojiText text={target.text} emojis={target.emojis} />
                </span>
              )}
            </Link>
          )}
        </span>
      )}
    </span>
  );
}

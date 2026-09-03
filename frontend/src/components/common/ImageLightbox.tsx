import { type MouseEvent as ReactMouseEvent, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import TwemojiEmoji from "./TwemojiEmoji";
import styles from "./ImageLightbox.module.css";

interface ImageLightboxProps {
  /** 表示中の画像URL。null なら非表示。 */
  src: string | null;
  onClose: () => void;
  sensitive?: boolean;
  onPrevious?: () => void;
  onNext?: () => void;
}

const SWIPE_THRESHOLD_PX = 50;
const DRAG_THRESHOLD_PX = 6;
const MIN_SCALE = 1;
const MAX_SCALE = 5;
const WHEEL_ZOOM_SPEED = 0.0015;

function clampScale(scale: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
}

function touchDistance(a: Touch, b: Touch): number {
  return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY);
}

/**
 * 添付画像クリック時のページ内ライトボックス表示（#64, #153）。
 *
 * オーバーレイ・Escで閉じ、複数画像ではボタン・左右矢印キー・スワイプで移動する。
 * ホイール／ピンチで100〜500%ズーム、ズーム中はドラッグ／指1本でビューポート移動する。
 * 表示画像はアップロード時の最大サイズに合わせ、CSS上で1024×1024pxを上限とする。
 */
export default function ImageLightbox({
  src,
  onClose,
  sensitive = false,
  onPrevious,
  onNext,
}: ImageLightboxProps) {
  const { t } = useTranslation();
  const [revealed, setRevealed] = useState(false);
  const [scale, setScale] = useState(1);
  const [translate, setTranslate] = useState({ x: 0, y: 0 });
  const [dragging, setDragging] = useState(false);

  const overlayRef = useRef<HTMLDivElement>(null);
  const imageWrapRef = useRef<HTMLDivElement>(null);

  // ネイティブイベントリスナー内で最新値を同期的に読むためのref（stale closure回避）。
  const scaleRef = useRef(scale);
  const translateRef = useRef(translate);
  useEffect(() => {
    scaleRef.current = scale;
  }, [scale]);
  useEffect(() => {
    translateRef.current = translate;
  }, [translate]);

  // クリック（onClose）を抑制するフラグ。ドラッグ/パン/ピンチ操作の終わりに
  // 発火するclickイベントで誤って閉じないようにする。
  const suppressClickRef = useRef(false);

  // NSFWぼかし未解除の間はズーム不可（解除ボタンごと拡大されてしまうのを防ぐ）。
  const zoomAllowedRef = useRef(!sensitive || revealed);
  useEffect(() => {
    zoomAllowedRef.current = !sensitive || revealed;
  }, [sensitive, revealed]);

  // ブラウザバックで閉じるための、pushState済みフラグ。
  const historyPushedRef = useRef(false);

  // 開いた時点（src: null→値）で1回だけhistoryにエントリを積む。
  useEffect(() => {
    if (src && !historyPushedRef.current) {
      historyPushedRef.current = true;
      window.history.pushState({ imageLightbox: true }, "");
    } else if (!src) {
      historyPushedRef.current = false;
    }
  }, [src]);

  useEffect(() => {
    function onPopState() {
      if (historyPushedRef.current) {
        historyPushedRef.current = false;
        onClose();
      }
    }
    window.addEventListener("popstate", onPopState);
    return () => window.removeEventListener("popstate", onPopState);
  }, [onClose]);

  /** Escや閉じるボタン等、UI操作からの終了。積んだhistoryエントリを消費させて閉じる。 */
  function requestClose() {
    if (historyPushedRef.current) {
      window.history.back();
    } else {
      onClose();
    }
  }

  useEffect(() => {
    if (!src) return;
    setRevealed(false);
    setScale(1);
    setTranslate({ x: 0, y: 0 });

    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        requestClose();
      } else if (e.key === "ArrowLeft" && onPrevious) {
        e.preventDefault();
        onPrevious();
      } else if (e.key === "ArrowRight" && onNext) {
        e.preventDefault();
        onNext();
      }
    }

    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [src, onPrevious, onNext]);

  // ホイール／ドラッグ／タッチ（ピンチ・パン・スワイプ）のジェスチャ処理。
  // React合成イベントのonWheel/onTouchはpassive登録のためpreventDefaultが効かず、
  // 背面タイムラインのスクロールを止められない。ネイティブリスナーで明示的に非passive登録する。
  useEffect(() => {
    const overlay = overlayRef.current;
    const imageWrap = imageWrapRef.current;
    if (!src || !overlay || !imageWrap) return;

    // 移動位置の制約:「等倍率のときの矩形（画面中央・imageWrapの素のサイズ）が
    // 常に現在の拡大後矩形に内包される」。scaleが1に近づくほど許容範囲が0に収束するため、
    // ズームアウト時に不自然なジャンプなく自然に中央へ戻る。
    function clampTranslate(tx: number, ty: number, scale: number) {
      const maxX = ((scale - 1) * imageWrap!.offsetWidth) / 2;
      const maxY = ((scale - 1) * imageWrap!.offsetHeight) / 2;
      return {
        x: Math.min(maxX, Math.max(-maxX, tx)),
        y: Math.min(maxY, Math.max(-maxY, ty)),
      };
    }

    function onWheel(e: WheelEvent) {
      e.preventDefault();
      if (!zoomAllowedRef.current) return;
      setScale((prev) => {
        const next = clampScale(prev * (1 - e.deltaY * WHEEL_ZOOM_SPEED));
        setTranslate((t) => clampTranslate(t.x, t.y, next));
        return next;
      });
    }

    // --- マウスドラッグ（ズーム中のみパン） ---
    let mouseDragActive = false;
    let mouseDidDrag = false;
    let mouseStart = { x: 0, y: 0 };
    let mouseStartTranslate = { x: 0, y: 0 };

    function onMouseDown(e: MouseEvent) {
      if (e.button !== 0 || scaleRef.current <= 1) return;
      e.preventDefault();
      mouseDragActive = true;
      mouseDidDrag = false;
      mouseStart = { x: e.clientX, y: e.clientY };
      mouseStartTranslate = { ...translateRef.current };
      suppressClickRef.current = false;
    }

    function onMouseMove(e: MouseEvent) {
      if (!mouseDragActive) return;
      const dx = e.clientX - mouseStart.x;
      const dy = e.clientY - mouseStart.y;
      if (!mouseDidDrag && (Math.abs(dx) > DRAG_THRESHOLD_PX || Math.abs(dy) > DRAG_THRESHOLD_PX)) {
        mouseDidDrag = true;
        suppressClickRef.current = true;
        setDragging(true);
      }
      if (mouseDidDrag) {
        setTranslate(
          clampTranslate(mouseStartTranslate.x + dx, mouseStartTranslate.y + dy, scaleRef.current),
        );
      }
    }

    function onMouseUp() {
      mouseDragActive = false;
      mouseDidDrag = false;
      setDragging(false);
    }

    // --- タッチ（ピンチズーム／1本指パン・スワイプ） ---
    let pinchStartDist: number | null = null;
    let pinchStartScale = 1;
    let touchStartX: number | null = null;
    let panStart: { x: number; y: number; tx: number; ty: number } | null = null;
    let touchDidDrag = false;

    function onTouchStart(e: TouchEvent) {
      if (e.touches.length === 2) {
        pinchStartDist = touchDistance(e.touches[0], e.touches[1]);
        pinchStartScale = scaleRef.current;
        touchStartX = null;
        panStart = null;
      } else if (e.touches.length === 1) {
        const t = e.touches[0];
        touchStartX = t.clientX;
        panStart = { x: t.clientX, y: t.clientY, tx: translateRef.current.x, ty: translateRef.current.y };
        touchDidDrag = false;
        suppressClickRef.current = false;
      }
    }

    function onTouchMove(e: TouchEvent) {
      if (e.touches.length === 2 && pinchStartDist !== null) {
        e.preventDefault();
        if (!zoomAllowedRef.current) return;
        const dist = touchDistance(e.touches[0], e.touches[1]);
        const next = clampScale(pinchStartScale * (dist / pinchStartDist));
        setScale(next);
        suppressClickRef.current = true;
        setTranslate((t) => clampTranslate(t.x, t.y, next));
        return;
      }
      if (e.touches.length === 1 && panStart) {
        const t = e.touches[0];
        const dx = t.clientX - panStart.x;
        const dy = t.clientY - panStart.y;
        if (!touchDidDrag && (Math.abs(dx) > DRAG_THRESHOLD_PX || Math.abs(dy) > DRAG_THRESHOLD_PX)) {
          touchDidDrag = true;
        }
        if (scaleRef.current > 1) {
          e.preventDefault();
          suppressClickRef.current = true;
          setTranslate(clampTranslate(panStart.tx + dx, panStart.ty + dy, scaleRef.current));
        }
      }
    }

    function onTouchEnd(e: TouchEvent) {
      if (e.touches.length < 2) {
        pinchStartDist = null;
      }
      if (e.touches.length === 0) {
        if (scaleRef.current === 1 && !touchDidDrag && touchStartX !== null) {
          const endX = e.changedTouches[0]?.clientX;
          if (endX !== undefined) {
            const delta = endX - touchStartX;
            if (delta > SWIPE_THRESHOLD_PX) onPrevious?.();
            if (delta < -SWIPE_THRESHOLD_PX) onNext?.();
          }
        }
        touchStartX = null;
        panStart = null;
        touchDidDrag = false;
      }
    }

    overlay.addEventListener("wheel", onWheel, { passive: false });
    imageWrap.addEventListener("mousedown", onMouseDown);
    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
    overlay.addEventListener("touchstart", onTouchStart, { passive: true });
    overlay.addEventListener("touchmove", onTouchMove, { passive: false });
    overlay.addEventListener("touchend", onTouchEnd, { passive: true });

    return () => {
      overlay.removeEventListener("wheel", onWheel);
      imageWrap.removeEventListener("mousedown", onMouseDown);
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
      overlay.removeEventListener("touchstart", onTouchStart);
      overlay.removeEventListener("touchmove", onTouchMove);
      overlay.removeEventListener("touchend", onTouchEnd);
    };
  }, [src, onPrevious, onNext]);

  if (!src) return null;

  function handleClick(e: ReactMouseEvent) {
    e.stopPropagation();
    if (suppressClickRef.current) {
      suppressClickRef.current = false;
      return;
    }
    requestClose();
  }

  const zoomed = scale > 1;
  const wrapStyle =
    scale !== 1 || translate.x !== 0 || translate.y !== 0
      ? { transform: `translate(${translate.x}px, ${translate.y}px) scale(${scale})` }
      : undefined;

  return (
    <div className={styles.overlay} onClick={handleClick} ref={overlayRef}>
      <button className={styles.close} onClick={(e) => { e.stopPropagation(); requestClose(); }} aria-label={t("common:close")}>
        ×
      </button>
      {onPrevious && (
        <button
          className={`${styles.pageButton} ${styles.previous}`}
          onClick={(e) => {
            e.stopPropagation();
            onPrevious();
          }}
          aria-label={t("common:previousImage")}
        >
          ‹
        </button>
      )}
      <div
        className={`${styles.imageWrap} ${zoomed ? styles.zoomed : ""} ${dragging ? styles.dragging : ""}`}
        onClick={handleClick}
        style={wrapStyle}
        ref={imageWrapRef}
      >
        <img
          src={src}
          alt=""
          draggable={false}
          className={`${styles.image} ${sensitive && !revealed ? styles.blurred : ""}`}
        />
        {sensitive && !revealed && (
          <button
            className={styles.reveal}
            aria-label={t("common:sensitiveImageReveal")}
            onClick={(e) => {
              e.stopPropagation();
              setRevealed(true);
            }}
          >
            <TwemojiEmoji emoji="👀" />
          </button>
        )}
      </div>
      {onNext && (
        <button
          className={`${styles.pageButton} ${styles.next}`}
          onClick={(e) => {
            e.stopPropagation();
            onNext();
          }}
          aria-label={t("common:nextImage")}
        >
          ›
        </button>
      )}
    </div>
  );
}

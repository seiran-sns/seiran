import {
  type MouseEvent as ReactMouseEvent,
  type TouchEvent as ReactTouchEvent,
  type TransitionEvent as ReactTransitionEvent,
  useEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import TwemojiEmoji from "./TwemojiEmoji";
import styles from "./ImageLightbox.module.css";
import RemoteImg from "./RemoteImg";

/** ページングのスワイプ中に、画面端から追従して現れる前後の画像。 */
export interface LightboxNeighbor {
  src: string;
  sensitive?: boolean;
}

interface ImageLightboxProps {
  /** 表示中の画像URL。null なら非表示。 */
  src: string | null;
  onClose: () => void;
  sensitive?: boolean;
  onPrevious?: () => void;
  onNext?: () => void;
  previousImage?: LightboxNeighbor;
  nextImage?: LightboxNeighbor;
}

const SWIPE_THRESHOLD_PX = 50;
const DRAG_THRESHOLD_PX = 6;
const MIN_SCALE = 1;
const MAX_SCALE = 5;
const WHEEL_ZOOM_SPEED = 0.0015;
/** 移動先の画像が無い方向へのスワイプで、指の移動量に対して画像を動かす割合（バネの抵抗感）。 */
const SWIPE_RUBBER_BAND = 0.3;
/** パン可能範囲の端に着いているとみなす誤差（px）。 */
const EDGE_EPSILON_PX = 0.5;
/** 前後の画像を置く、表示中の画像との間隔（px）。前後の画像は画面幅＋この間隔だけ横にずらして置く。 */
const PAGE_GAP_PX = 16;
/** ページング確定のアニメーションが終わらなかった場合に、切り替えを確定させるまでの時間（ms）。 */
const PAGE_COMMIT_FALLBACK_MS = 400;
/** 縦スワイプで閉じるのに必要な移動量（px）。 */
const DISMISS_THRESHOLD_PX = 80;
/** 縦スワイプで背景が最も薄くなるまでの移動量（px）。 */
const DISMISS_FADE_DISTANCE_PX = 300;
const OVERLAY_ALPHA = 0.85;
/**
 * タッチ操作の後にブラウザが合成するclickを、タップとみなして無視する期間（ms）。
 * スマホではタップで閉じず、✕ボタンか縦スワイプで閉じる（横スワイプの操作中の誤タップで閉じないため）。
 */
const TOUCH_CLICK_GUARD_MS = 800;
/** ダブルタップとみなす2回のタップの間隔（ms）と位置のずれ（px）の上限。 */
const DOUBLE_TAP_INTERVAL_MS = 300;
const DOUBLE_TAP_SLOP_PX = 30;
/** ダブルタップの1段目の拡大率。これ未満ならここへ、これ以上なら最大へ、最大なら等倍へ戻す。 */
const DOUBLE_TAP_SCALE = 2.25;

/** ダブルタップ後の拡大率。 */
function doubleTapTargetScale(current: number): number {
  if (current >= MAX_SCALE - 0.001) return MIN_SCALE;
  if (current < DOUBLE_TAP_SCALE) return DOUBLE_TAP_SCALE;
  return MAX_SCALE;
}

function clampScale(scale: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
}

type Point = { x: number; y: number };

/** 指を離した後に確定させる操作（画像の切り替えか、閉じるか）。 */
type CommitKind = "next" | "previous" | "dismiss";

function touchDistance(a: Touch, b: Touch): number {
  return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY);
}

function touchMidpoint(a: Touch, b: Touch): Point {
  return { x: (a.clientX + b.clientX) / 2, y: (a.clientY + b.clientY) / 2 };
}

/**
 * 1回のタッチ操作の状態。指1本の操作はモード（スクロールかページングか）を動き出した時点で
 * 1度だけ決め、操作の途中では切り替えない（スクロールで端に着いてもそのまま次の画像へは行かない）。
 */
type TouchGesture =
  | { kind: "idle" }
  /** 指1本が触れたが、まだ動きの閾値を超えていない。 */
  | { kind: "pending"; start: Point; startTranslate: Point }
  /** 拡大画像のスクロール。 */
  | { kind: "pan"; start: Point; startTranslate: Point }
  /** 画像のページング。画像は指に横方向だけ追従し、移動先が無ければ抵抗をつけて追従する。 */
  | { kind: "page"; startX: number; direction: "next" | "previous"; hasTarget: boolean }
  /** 縦スワイプで閉じる操作。画像は指に縦方向だけ追従し、背景が薄くなる。 */
  | { kind: "dismiss"; startY: number }
  /**
   * 2本指のピンチ。開始時に2本指の中点の下にあった画像上の点が、常に現在の中点の下に来るように
   * 拡大率と位置を決める（ピンチしながらの平行移動にも追従する）。
   */
  | {
      kind: "pinch";
      startDist: number;
      startScale: number;
      startMid: Point;
      startTranslate: Point;
      /** imageWrapの変形前（translate/scale無し）のレイアウト上の中心。 */
      layoutCenter: Point;
    };

/**
 * 添付画像クリック時のページ内ライトボックス表示（#64, #153）。
 *
 * オーバーレイ・Escで閉じ、複数画像ではボタン・左右矢印キー・スワイプで移動する。
 * ホイール／ピンチで100〜500%ズーム、ズーム中はドラッグ／指1本でビューポート移動する。
 * スワイプは、その向きの先に拡大ではみ出した部分が残っていればスクロール、残っていなければ
 * 横はページング、縦は閉じる操作になる。タップでは閉じない（PCのクリックでは閉じる）。
 * 画像上のダブルタップで、タップした点を中心に 100%→225%→500%→100% と拡大率を切り替える。
 * 表示画像はアップロード時の最大サイズに合わせ、CSS上で1024×1024pxを上限とする。
 */
export default function ImageLightbox({
  src,
  onClose,
  sensitive = false,
  onPrevious,
  onNext,
  previousImage,
  nextImage,
}: ImageLightboxProps) {
  const { t } = useTranslation();
  const [revealed, setRevealed] = useState(false);
  const [scale, setScale] = useState(1);
  const [translate, setTranslate] = useState({ x: 0, y: 0 });
  const [dragging, setDragging] = useState(false);
  /** ページング／閉じるスワイプ中に画像をずらす量（px）。ページングはx、閉じるはyだけを使う。 */
  const [swipeOffset, setSwipeOffset] = useState<Point>({ x: 0, y: 0 });
  /** 指を離した後、ずらした画像を元の位置へアニメーションで戻している最中。 */
  const [settling, setSettling] = useState(false);
  /** ダブルタップで拡大率を切り替えるアニメーションの最中。 */
  const [zoomAnimating, setZoomAnimating] = useState(false);
  /**
   * スワイプ確定のアニメーション中。ページングは表示中の画像が画面外へ出て隣の画像が中央へ来る、
   * 閉じるは画像が上下へ飛び去り背景が消える。アニメーションが終わった時点で切り替える／閉じる。
   */
  const [committing, setCommitting] = useState<CommitKind | null>(null);
  // アニメーション終了とフォールバックのタイマーの両方から呼ばれても1回だけ切り替えるため、同期的に読み書きする。
  const committingRef = useRef<CommitKind | null>(null);
  const closeRequestedRef = useRef(false);

  const overlayRef = useRef<HTMLDivElement>(null);
  const imageWrapRef = useRef<HTMLDivElement>(null);

  // ネイティブイベントリスナー内で最新値を同期的に読むためのref（stale closure回避）。
  const scaleRef = useRef(scale);
  const translateRef = useRef(translate);
  const swipeOffsetRef = useRef(swipeOffset);
  useEffect(() => {
    scaleRef.current = scale;
  }, [scale]);
  useEffect(() => {
    translateRef.current = translate;
  }, [translate]);
  useEffect(() => {
    swipeOffsetRef.current = swipeOffset;
  }, [swipeOffset]);

  // 親は毎レンダーで新しい関数を渡してくるので、ジェスチャ処理のeffectの依存に入れると
  // 操作の途中でリスナーが張り直され、ジェスチャ状態が失われる。refで最新値を読む。
  const onPreviousRef = useRef(onPrevious);
  const onNextRef = useRef(onNext);
  useEffect(() => {
    onPreviousRef.current = onPrevious;
    onNextRef.current = onNext;
  }, [onPrevious, onNext]);

  // クリック（onClose）を抑制するフラグ。ドラッグ/パン/ピンチ操作の終わりに
  // 発火するclickイベントで誤って閉じないようにする。
  const suppressClickRef = useRef(false);
  /** 最後にタッチ操作が終わった時刻（performance.now()）。直後のclickはタップ由来として閉じない。 */
  const lastTouchEndRef = useRef(Number.NEGATIVE_INFINITY);

  // effect内のリスナーから最新のrequestCloseを呼ぶためのref。
  const requestCloseRef = useRef<() => void>(() => {});

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
  requestCloseRef.current = requestClose;

  useEffect(() => {
    if (!src) return;
    setRevealed(false);
    setScale(1);
    setTranslate({ x: 0, y: 0 });
    setSwipeOffset({ x: 0, y: 0 });
    setSettling(false);
    setZoomAnimating(false);
    setCommitting(null);
    committingRef.current = null;
    closeRequestedRef.current = false;

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
    function maxTranslateX(scale: number) {
      return ((scale - 1) * imageWrap!.offsetWidth) / 2;
    }

    function maxTranslateY(scale: number) {
      return ((scale - 1) * imageWrap!.offsetHeight) / 2;
    }

    function clampTranslate(tx: number, ty: number, scale: number) {
      const maxX = maxTranslateX(scale);
      const maxY = maxTranslateY(scale);
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

    // --- タッチ（ピンチズーム／指1本のスクロール・ページング） ---
    let gesture: TouchGesture = { kind: "idle" };
    /** 直前の（動かさずに離した）タップ。次のタップと合わせてダブルタップを判定する。 */
    let lastTap: { time: number; x: number; y: number } | null = null;

    /**
     * 画像上のダブルタップで、タップした点を指の下に保ったまま拡大率を切り替える
     * （225%未満→225%、225%以上500%未満→500%、500%→100%）。
     */
    function zoomAtPoint(x: number, y: number) {
      if (!zoomAllowedRef.current) return;
      const rect = imageWrap!.getBoundingClientRect();
      if (x < rect.left || x > rect.right || y < rect.top || y > rect.bottom) return;
      const s = scaleRef.current;
      const t = translateRef.current;
      const nextScale = doubleTapTargetScale(s);
      const { x: cx, y: cy } = layoutCenter();
      // タップした点の画像上の位置（中心基準・等倍の座標）。拡大後も同じ点が指の下に来るようにする。
      const px = (x - cx - t.x) / s;
      const py = (y - cy - t.y) / s;
      setZoomAnimating(true);
      setScale(nextScale);
      setTranslate(clampTranslate(x - cx - nextScale * px, y - cy - nextScale * py, nextScale));
    }

    /** 動かさずに離したタップを記録し、直前のタップと合わせてダブルタップなら拡大する。 */
    function handleTap(x: number, y: number) {
      const now = performance.now();
      if (
        lastTap &&
        now - lastTap.time <= DOUBLE_TAP_INTERVAL_MS &&
        Math.hypot(x - lastTap.x, y - lastTap.y) <= DOUBLE_TAP_SLOP_PX
      ) {
        lastTap = null;
        zoomAtPoint(x, y);
        return;
      }
      lastTap = { time: now, x, y };
    }

    /**
     * imageWrapの変形前（translate/scale無し）のレイアウト上の中心。transformは中心基準のscaleなので、
     * 画像上の点の画面座標 = この中心 + translate + scale × (中心からの等倍の位置)。
     * getBoundingClientRectはアニメーション途中の変形後の矩形を返すので、変形の影響を受けない
     * offset系の値（offsetParentはposition: fixedのoverlay）から求める。
     */
    function layoutCenter(): Point {
      const overlayRect = overlay!.getBoundingClientRect();
      return {
        x: overlayRect.left + imageWrap!.offsetLeft + imageWrap!.offsetWidth / 2,
        y: overlayRect.top + imageWrap!.offsetTop + imageWrap!.offsetHeight / 2,
      };
    }

    function beginPinch(a: Touch, b: Touch) {
      // ページング／閉じるでずらしていた分は捨て、拡大の基準にしない。
      setSwipeOffset({ x: 0, y: 0 });
      gesture = {
        kind: "pinch",
        startDist: touchDistance(a, b),
        startScale: scaleRef.current,
        startMid: touchMidpoint(a, b),
        startTranslate: { ...translateRef.current },
        layoutCenter: layoutCenter(),
      };
      suppressClickRef.current = true;
    }

    /** 指1本の動き出しで、スクロールか、ページング（横）／閉じる（縦）かを決める。 */
    function decideSingleTouchMode(dx: number, dy: number, start: Point, startTranslate: Point) {
      if (Math.abs(dx) <= Math.abs(dy)) {
        // 指を下へ動かすと、画像の上側に隠れた部分が見えてくる。
        const maxY = maxTranslateY(scaleRef.current);
        const hiddenRemains =
          dy > 0
            ? startTranslate.y < maxY - EDGE_EPSILON_PX
            : startTranslate.y > -maxY + EDGE_EPSILON_PX;
        gesture = hiddenRemains
          ? { kind: "pan", start, startTranslate }
          : { kind: "dismiss", startY: start.y };
        setSettling(false);
        return;
      }
      // 指を左へ動かす（左スワイプ）と、画像の右側に隠れた部分が見えてくる。
      const direction = dx < 0 ? "next" : "previous";
      const maxX = maxTranslateX(scaleRef.current);
      const hiddenRemains =
        direction === "next"
          ? startTranslate.x > -maxX + EDGE_EPSILON_PX
          : startTranslate.x < maxX - EDGE_EPSILON_PX;
      if (hiddenRemains) {
        gesture = { kind: "pan", start, startTranslate };
        return;
      }
      const hasTarget = direction === "next" ? !!onNextRef.current : !!onPreviousRef.current;
      gesture = { kind: "page", startX: start.x, direction, hasTarget };
      setSettling(false);
    }

    function onTouchStart(e: TouchEvent) {
      // ページング確定のアニメーション中の操作は受け付けない（切り替え後の画像に引き継ぐ状態が無い）。
      if (committingRef.current) {
        gesture = { kind: "idle" };
        return;
      }
      // 戻り・拡大のアニメーション中に触れたら、アニメーションを打ち切って指に追従させる
      // （transitionが残ると指の動きに遅れてついてくる）。
      setSettling(false);
      setZoomAnimating(false);
      if (e.touches.length === 2) {
        beginPinch(e.touches[0], e.touches[1]);
      } else if (e.touches.length === 1) {
        const t = e.touches[0];
        gesture = {
          kind: "pending",
          start: { x: t.clientX, y: t.clientY },
          startTranslate: { ...translateRef.current },
        };
        suppressClickRef.current = false;
      }
    }

    function onTouchMove(e: TouchEvent) {
      // 背面のスクロール・ブラウザ標準ジェスチャを止める。
      e.preventDefault();

      if (gesture.kind === "pinch") {
        if (e.touches.length < 2 || !zoomAllowedRef.current) return;
        const g = gesture;
        const nextScale = clampScale(
          g.startScale * (touchDistance(e.touches[0], e.touches[1]) / g.startDist),
        );
        const mid = touchMidpoint(e.touches[0], e.touches[1]);
        // 開始時に中点の下にあった画像上の点（imageWrap中心基準・等倍の座標）。
        const px = (g.startMid.x - g.layoutCenter.x - g.startTranslate.x) / g.startScale;
        const py = (g.startMid.y - g.layoutCenter.y - g.startTranslate.y) / g.startScale;
        setScale(nextScale);
        setTranslate(
          clampTranslate(
            mid.x - g.layoutCenter.x - nextScale * px,
            mid.y - g.layoutCenter.y - nextScale * py,
            nextScale,
          ),
        );
        return;
      }

      if (e.touches.length !== 1) return;
      const t = e.touches[0];

      if (gesture.kind === "pending") {
        const dx = t.clientX - gesture.start.x;
        const dy = t.clientY - gesture.start.y;
        if (Math.abs(dx) <= DRAG_THRESHOLD_PX && Math.abs(dy) <= DRAG_THRESHOLD_PX) return;
        suppressClickRef.current = true;
        decideSingleTouchMode(dx, dy, gesture.start, gesture.startTranslate);
      }

      if (gesture.kind === "pan") {
        const dx = t.clientX - gesture.start.x;
        const dy = t.clientY - gesture.start.y;
        setTranslate(
          clampTranslate(gesture.startTranslate.x + dx, gesture.startTranslate.y + dy, scaleRef.current),
        );
      } else if (gesture.kind === "page") {
        const dx = t.clientX - gesture.startX;
        // スワイプを始めた向きと逆へは動かさない。
        const raw = gesture.direction === "next" ? Math.min(0, dx) : Math.max(0, dx);
        setSwipeOffset({ x: gesture.hasTarget ? raw : raw * SWIPE_RUBBER_BAND, y: 0 });
      } else if (gesture.kind === "dismiss") {
        setSwipeOffset({ x: 0, y: t.clientY - gesture.startY });
      }
    }

    /**
     * ページング／閉じるスワイプを終える。閾値を超えていれば画像を切り替える／閉じる。
     * そうでなければ元の位置へバネのように戻す。
     */
    function finishSwipe(commit: boolean) {
      const offset = swipeOffsetRef.current;
      if (gesture.kind === "page") {
        if (commit && gesture.hasTarget && Math.abs(offset.x) > SWIPE_THRESHOLD_PX) {
          // 表示中の画像を画面外へ、隣の画像（画面幅＋間隔だけ横に置いてある）を中央へ送る。
          const distance = overlay!.clientWidth + PAGE_GAP_PX;
          committingRef.current = gesture.direction;
          setSettling(false);
          setCommitting(gesture.direction);
          setSwipeOffset({ x: gesture.direction === "next" ? -distance : distance, y: 0 });
          return;
        }
      } else if (gesture.kind === "dismiss") {
        if (commit && Math.abs(offset.y) > DISMISS_THRESHOLD_PX) {
          // スワイプした向きへ画面の高さ分飛ばしてから閉じる。
          const distance = overlay!.clientHeight + PAGE_GAP_PX;
          committingRef.current = "dismiss";
          setSettling(false);
          setCommitting("dismiss");
          setSwipeOffset({ x: 0, y: offset.y > 0 ? distance : -distance });
          return;
        }
      } else {
        return;
      }
      if (offset.x !== 0 || offset.y !== 0) {
        setSettling(true);
        setSwipeOffset({ x: 0, y: 0 });
      }
    }

    function onTouchEnd(e: TouchEvent) {
      if (e.touches.length === 1) {
        // ピンチから指1本が残ったら、残った指でのスクロールとして続ける（ページングはしない）。
        const t = e.touches[0];
        finishSwipe(false);
        gesture = {
          kind: "pan",
          start: { x: t.clientX, y: t.clientY },
          startTranslate: { ...translateRef.current },
        };
        return;
      }
      if (e.touches.length === 0) {
        lastTouchEndRef.current = performance.now();
        const tap = e.changedTouches[0];
        if (gesture.kind === "pending" && tap) {
          handleTap(tap.clientX, tap.clientY);
        } else {
          lastTap = null;
        }
        finishSwipe(true);
        gesture = { kind: "idle" };
      }
    }

    function onTouchCancel() {
      lastTouchEndRef.current = performance.now();
      lastTap = null;
      finishSwipe(false);
      gesture = { kind: "idle" };
    }

    overlay.addEventListener("wheel", onWheel, { passive: false });
    imageWrap.addEventListener("mousedown", onMouseDown);
    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
    overlay.addEventListener("touchstart", onTouchStart, { passive: true });
    overlay.addEventListener("touchmove", onTouchMove, { passive: false });
    overlay.addEventListener("touchend", onTouchEnd, { passive: true });
    overlay.addEventListener("touchcancel", onTouchCancel, { passive: true });

    return () => {
      overlay.removeEventListener("wheel", onWheel);
      imageWrap.removeEventListener("mousedown", onMouseDown);
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
      overlay.removeEventListener("touchstart", onTouchStart);
      overlay.removeEventListener("touchmove", onTouchMove);
      overlay.removeEventListener("touchend", onTouchEnd);
      overlay.removeEventListener("touchcancel", onTouchCancel);
    };
  }, [src]);

  /**
   * スワイプ確定のアニメーションを終え、閉じる、または親に切り替えを伝える。切り替えでは
   * ずらし量・拡大率も同じレンダーで戻す（src切り替えのeffectを待つと、新しい画像が画面外・
   * 旧拡大率のまま1フレーム描画される）。
   */
  function completeCommit() {
    const direction = committingRef.current;
    if (!direction) return;
    if (direction === "dismiss") {
      // 閉じ終わるまで（popstate経由で非表示になるまで）committingを残し、背景を消したまま操作も受け付けない。
      // 次に開いたときにsrc切り替えのeffectで戻す。transitionendとタイマーの両方から呼ばれるので1回だけ閉じる。
      if (closeRequestedRef.current) return;
      closeRequestedRef.current = true;
      requestCloseRef.current();
      return;
    }
    committingRef.current = null;
    setCommitting(null);
    setSwipeOffset({ x: 0, y: 0 });
    setScale(1);
    setTranslate({ x: 0, y: 0 });
    setRevealed(false);
    if (direction === "next") onNextRef.current?.();
    else onPreviousRef.current?.();
  }
  const completeCommitRef = useRef(completeCommit);
  completeCommitRef.current = completeCommit;

  // transitionendが来ない場合（タブが裏に回った等）でも切り替えを確定させる。
  useEffect(() => {
    if (!committing) return;
    const id = window.setTimeout(() => completeCommitRef.current(), PAGE_COMMIT_FALLBACK_MS);
    return () => window.clearTimeout(id);
  }, [committing]);

  if (!src) return null;

  function handleClick(e: ReactMouseEvent) {
    e.stopPropagation();
    if (suppressClickRef.current) {
      suppressClickRef.current = false;
      return;
    }
    if (performance.now() - lastTouchEndRef.current < TOUCH_CLICK_GUARD_MS) return;
    requestClose();
  }

  // ライトボックスはノートの中（Reactツリー上はタイムラインの子孫）に描画されるため、
  // React合成イベントのタッチがホームのタブ切り替えスワイプ（`useSwipe`）まで伝播する。ここで止める。
  function stopTouch(e: ReactTouchEvent) {
    e.stopPropagation();
  }

  const zoomed = scale > 1;
  const offsetX = translate.x + swipeOffset.x;
  const offsetY = translate.y + swipeOffset.y;
  const wrapStyle =
    scale !== 1 || offsetX !== 0 || offsetY !== 0
      ? { transform: `translate(${offsetX}px, ${offsetY}px) scale(${scale})` }
      : undefined;
  // 閉じるスワイプの移動量に応じて背景を薄くし、離せば閉じることを示す。閉じる確定後は透明まで消す。
  const overlayAlpha =
    committing === "dismiss"
      ? 0
      : OVERLAY_ALPHA * (1 - 0.7 * Math.min(1, Math.abs(swipeOffset.y) / DISMISS_FADE_DISTANCE_PX));
  const overlayStyle =
    swipeOffset.y !== 0 ? { backgroundColor: `rgba(10, 14, 26, ${overlayAlpha})` } : undefined;
  const transitionClass = committing
    ? styles.committing
    : zoomAnimating
      ? styles.zooming
      : settling
        ? styles.settling
        : "";

  // 表示中の画像のtransformアニメーションの終わり（子のimgのアニメーション等は除く）。
  function handleWrapTransitionEnd(e: ReactTransitionEvent) {
    if (e.target !== e.currentTarget || e.propertyName !== "transform") return;
    if (committingRef.current) {
      completeCommit();
    } else {
      setSettling(false);
      setZoomAnimating(false);
    }
  }

  return (
    <div
      className={`${styles.overlay} ${transitionClass}`}
      style={overlayStyle}
      onClick={handleClick}
      onTouchStart={stopTouch}
      onTouchMove={stopTouch}
      onTouchEnd={stopTouch}
      onTouchCancel={stopTouch}
      ref={overlayRef}
    >
      {/* 前後の画像。画面幅＋間隔だけ横に置き、ページングのスワイプ量だけ一緒に動かす。
          常に描画しておくことで、スワイプを始めた時点で読み込みが済んでいるようにする。
          ✕ボタン・表示中の画像より手前に出ないよう、overlayの先頭に置く。 */}
      {(
        [
          [previousImage, -1],
          [nextImage, 1],
        ] as const
      ).map(
        ([image, side]) =>
          image && (
            <div
              key={side}
              aria-hidden
              className={`${styles.neighbor} ${transitionClass}`}
              style={{
                transform: `translateX(calc(${side * 100}% + ${side * PAGE_GAP_PX + swipeOffset.x}px))`,
              }}
            >
              <div className={styles.imageWrap}>
                <RemoteImg
                  src={image.src}
                  alt=""
                  draggable={false}
                  className={`${styles.image} ${image.sensitive ? styles.blurred : ""}`}
                />
              </div>
            </div>
          ),
      )}
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
        className={`${styles.imageWrap} ${zoomed ? styles.zoomed : ""} ${dragging ? styles.dragging : ""} ${transitionClass}`}
        onClick={handleClick}
        onTransitionEnd={handleWrapTransitionEnd}
        style={wrapStyle}
        ref={imageWrapRef}
      >
        <RemoteImg
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

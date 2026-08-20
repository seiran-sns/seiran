import { CSSProperties, useState } from "react";
import { useTranslation } from "react-i18next";
import { NoteAttachment } from "../../api/client";
import ImageLightbox from "../common/ImageLightbox";
import TwemojiEmoji from "../common/TwemojiEmoji";
import HlsVideo from "./HlsVideo";
import styles from "./NoteCard.module.css";
import { mediaUrl } from "../../utils/mediaProxy";

interface NoteAttachmentsProps {
  attachments?: NoteAttachment[];
}

function isImage(att: NoteAttachment) {
  const isHls =
    att.mimeType === "application/vnd.apple.mpegurl" ||
    att.mimeType === "application/x-mpegURL";
  return !att.mimeType.startsWith("video/") && !att.mimeType.startsWith("audio/") && !isHls;
}

/** `attachments`配列内でのグローバルインデックス付きの添付（sensitive状態・lightbox連携に使う）。 */
interface IndexedAttachment {
  attachment: NoteAttachment;
  index: number;
}

/** 動画/音声を境目に画像を切り分けた単位。動画・音声は幅いっぱいでタイリング対象外のため、
 * 連続する画像だけを1つの`images`ブロックにまとめ、枚数に応じたレイアウトを適用する。 */
type Block =
  | { kind: "media"; item: IndexedAttachment }
  | { kind: "images"; items: IndexedAttachment[] };

function buildBlocks(attachments: NoteAttachment[]): Block[] {
  const blocks: Block[] = [];
  let current: IndexedAttachment[] = [];
  const flush = () => {
    if (current.length > 0) {
      blocks.push({ kind: "images", items: current });
      current = [];
    }
  };
  attachments.forEach((attachment, index) => {
    if (isImage(attachment)) {
      current.push({ attachment, index });
    } else {
      flush();
      blocks.push({ kind: "media", item: { attachment, index } });
    }
  });
  flush();
  return blocks;
}

/** 4枚以上ブロックの「残り」を2枚1行のグリッドへ分割する（呼び出し側で偶数個であることを保証する）。 */
function chunkPairs(items: IndexedAttachment[]): IndexedAttachment[][] {
  const rows: IndexedAttachment[][] = [];
  for (let i = 0; i < items.length; i += 2) {
    rows.push(items.slice(i, i + 2));
  }
  return rows;
}

const CELL_RADIUS = "8px";

/** ブロック全体をひとつの角丸矩形として見たときの4隅を、該当するタイルにだけ割り当てる。
 * `globalIndex`はブロック内の1始まり通し番号、`total`はブロックの総枚数。
 * 3枚パターンの左タイル・4枚以上パターンの最終行左タイルが「左下」を担当する点だけ特殊
 * （`total < 4`なら1枚目、そうでなければ`total - 1`枚目）。 */
function cornerRadius(globalIndex: number, total: number): CSSProperties {
  const topLeft = globalIndex === 1;
  const topRight = globalIndex === Math.min(total, 2);
  const bottomRight = globalIndex === total;
  const bottomLeft = globalIndex === (total < 4 ? 1 : total - 1);
  const r = (on: boolean) => (on ? CELL_RADIUS : "0");
  return {
    borderRadius: `${r(topLeft)} ${r(topRight)} ${r(bottomRight)} ${r(bottomLeft)}`,
  };
}

/** 実寸不明時のフォールバック値。どのタイルの箱サイズよりも十分大きい値にすることで、
 * `min(var(--nw) * 2, 箱サイズ)`が常に箱サイズ側に確定し、2倍キャップそのものを
 * 事実上無効化する（実寸不明なら通常のcontain/cover任せの拡大縮小になる）。 */
const UNKNOWN_NATURAL_SIZE_PX = 9999;

/** 画像の実寸をCSSカスタムプロパティとして渡す。CSSの`calc()`は長さ同士の除算ができない
 * （比率を直接計算できない）ため、「拡大率を2倍までに制限する」ロジックは比率計算ではなく
 * 「画像を当てはめる箱自体を実寸の2倍で頭打ちにしてから`object-fit`に丸投げする」方式で実現する
 * （各タイルのCSS参照）。
 *
 * リモート受信投稿（`media_file_id`を持たず`remote_url`をそのまま使う）は実寸を保存して
 * いないため、バックエンドは`width`/`height`を`0`で返す（`queries.rs`の`COALESCE(mf.width, 0)`）。
 * これを迂闊に小さい値へクランプすると「実寸2px」等として扱われ、2倍キャップにより
 * 画像が極小に押し込められてしまう（実際に発生した回帰）。0以下は「実寸不明」として
 * キャップ自体を無効化するのが正しい。 */
function naturalSizeVars(att: NoteAttachment): CSSProperties {
  const w = att.width > 0 ? att.width : UNKNOWN_NATURAL_SIZE_PX;
  const h = att.height > 0 ? att.height : UNKNOWN_NATURAL_SIZE_PX;
  return {
    "--nw": `${w}px`,
    "--nh": `${h}px`,
  } as CSSProperties;
}

/** 投稿に添付されたメディア（画像/動画/HLS/音声）一覧の表示。
 *
 * 画像は動画/音声を境目にブロック分割し、ブロック内の枚数（1/2/3/4枚以上）に応じて
 * 専用のタイルレイアウトを適用する（詳細は各render関数のコメント参照）。 */
export default function NoteAttachments({ attachments }: NoteAttachmentsProps) {
  const { t } = useTranslation();
  const [lightboxIndex, setLightboxIndex] = useState<number | null>(null);
  const [revealed, setRevealed] = useState<Set<number>>(() => new Set());

  if (!attachments || attachments.length === 0) return null;

  const images = attachments.filter(isImage);
  const blocks = buildBlocks(attachments);

  function reveal(index: number) {
    setRevealed((current) => new Set(current).add(index));
  }
  function hide(index: number) {
    setRevealed((current) => {
      const next = new Set(current);
      next.delete(index);
      return next;
    });
  }

  /** 1タイル分。センシティブぼかし/reveal、クリックでのlightbox起動は共通。
   * `cellClassName`がタイルの箱（サイズ・配置）、`imgClassName`が中の`<img>`のフィット方式を決める。
   * `radius`はブロック全体の4隅のうちこのタイルが担当する角（`cornerRadius`参照）。
   * セル（箱）側の`overflow: hidden`でクリップするため、NSFWぼかし（`filter: blur`）が
   * 掛かっていてもにじみごと角丸で切り取られる。 */
  function renderImageCell(
    item: IndexedAttachment,
    cellClassName: string,
    imgClassName: string,
    radius: CSSProperties,
  ) {
    const { attachment: att, index } = item;
    const isRevealed = revealed.has(index);
    const imageIndex = images.indexOf(att);
    return (
      <div key={index} className={cellClassName} style={radius}>
        <img
          src={mediaUrl(att.url)}
          alt=""
          style={naturalSizeVars(att)}
          className={`${imgClassName} ${att.isSensitive && !isRevealed ? styles.attachBlurred : ""}`}
          loading="lazy"
          onClick={(e) => {
            e.stopPropagation();
            if (att.isSensitive && isRevealed) {
              hide(index);
            } else {
              setLightboxIndex(imageIndex);
            }
          }}
        />
        {att.isSensitive && !isRevealed && (
          <button
            className={styles.sensitiveReveal}
            aria-label={t("common:sensitiveImageReveal")}
            onClick={(e) => {
              e.stopPropagation();
              reveal(index);
            }}
          >
            <TwemojiEmoji emoji="👀" />
          </button>
        )}
      </div>
    );
  }

  /** 1枚: 幅いっぱいの領域に、実寸2倍までズームし、縦がmax-heightを超えたら中央クロップする。 */
  function renderSingle(item: IndexedAttachment) {
    return (
      <div className={styles.imgBlockSingle}>
        {renderImageCell(item, styles.imgCellSingle, styles.imgFitCover, cornerRadius(1, 1))}
      </div>
    );
  }

  /** 2枚: 縦長タイル2つを横に並べる。各タイルは幅・高さとも固定で、実寸2倍までズームし
   * 箱を満たすようクロップする（`object-fit: cover`）。2倍キャップに掛かって箱に満たない
   * 場合だけ縦横とも中央揃えの余白になる。 */
  function renderPair(items: IndexedAttachment[]) {
    return (
      <div className={styles.imgRow}>
        {items.map((it, i) =>
          renderImageCell(it, styles.imgCellBig, styles.imgFitCropCapped, cornerRadius(i + 1, 2)),
        )}
      </div>
    );
  }

  /** 3枚: 左に1枚（2枚ブロックと同じ縦長タイル）、右を上下に分けて2枚。
   * `total`はブロック全体の枚数（4枚以上・奇数パターンの先頭3枚として呼ばれる場合は
   * ブロック全体の枚数、3枚単体ブロックの場合は3）で、角丸の4隅判定に使う。 */
  function renderTriple(items: [IndexedAttachment, IndexedAttachment, IndexedAttachment], total: number) {
    const [left, topRight, bottomRight] = items;
    return (
      <div className={styles.imgRow}>
        {renderImageCell(left, styles.imgCellBig, styles.imgFitCropCapped, cornerRadius(1, total))}
        <div className={styles.imgColSmall}>
          {renderImageCell(topRight, styles.imgCellSmallFull, styles.imgFitCropCapped, cornerRadius(2, total))}
          {renderImageCell(bottomRight, styles.imgCellSmallFull, styles.imgFitCropCapped, cornerRadius(3, total))}
        </div>
      </div>
    );
  }

  /** 4枚以上の「偶数枚パターン」: 小タイルを2枚1行で下に並べていく。
   * `total`/`startIndex`は角丸判定用のブロック全体枚数とこのグリッドの先頭がブロック内
   * 何枚目かを表す（奇数パターンでは先頭3枚が3枚ブロック扱いのため、グリッドは4枚目から）。 */
  function renderGrid(items: IndexedAttachment[], total: number, startIndex: number) {
    return (
      <div className={styles.imgGrid}>
        {chunkPairs(items).map((row, ri) => (
          <div className={styles.imgRow} key={ri}>
            {row.map((it, ci) =>
              renderImageCell(
                it,
                styles.imgCellSmallHalf,
                styles.imgFitCropCapped,
                cornerRadius(startIndex + ri * 2 + ci, total),
              ),
            )}
          </div>
        ))}
      </div>
    );
  }

  function renderImageBlock(items: IndexedAttachment[], blockKey: string) {
    const total = items.length;
    if (total === 1) return <div key={blockKey}>{renderSingle(items[0])}</div>;
    if (total === 2) return <div key={blockKey}>{renderPair(items)}</div>;
    if (total === 3) {
      return (
        <div key={blockKey}>
          {renderTriple(items as [IndexedAttachment, IndexedAttachment, IndexedAttachment], total)}
        </div>
      );
    }
    // 4枚以上: 奇数なら先頭3枚を3枚ブロックと同じ形式にし、残り（必ず偶数）をグリッドにする。
    if (total % 2 === 1) {
      const head = items.slice(0, 3) as [IndexedAttachment, IndexedAttachment, IndexedAttachment];
      const rest = items.slice(3);
      return (
        <div key={blockKey} className={styles.imgBlockStack}>
          {renderTriple(head, total)}
          {renderGrid(rest, total, 4)}
        </div>
      );
    }
    return <div key={blockKey}>{renderGrid(items, total, 1)}</div>;
  }

  return (
    <div className={styles.attachments}>
      {blocks.map((block, bi) => {
        if (block.kind === "images") {
          return renderImageBlock(block.items, `img-${bi}`);
        }
        const { attachment: att, index } = block.item;
        const isHls =
          att.mimeType === "application/vnd.apple.mpegurl" ||
          att.mimeType === "application/x-mpegURL";
        if (att.mimeType.startsWith("video/") || isHls) {
          // HLS(.m3u8)は `application/vnd.apple.mpegurl` が /proxy の許可Content-Type
          // （image/video/audio）に一致せず502になる上、プロキシ経由だとプレイリスト内の
          // セグメント相対パスもブラウザ側で正しく解決できない。Bsky動画CDNは
          // `access-control-allow-origin: *` を返しCORS制約が無いため、HLSのみ直接
          // 参照する（通常のmp4/webm添付は引き続きプロキシを経由する）。
          return (
            <HlsVideo
              key={index}
              src={isHls ? att.url : (mediaUrl(att.url) ?? att.url)}
              fallbackSrc={isHls ? undefined : att.url}
              mimeType={att.mimeType}
              poster={mediaUrl(att.thumbnailUrl)}
              isHls={isHls}
              isGif={att.isGif}
              className={styles.attachMedia}
              onClick={(e) => e.stopPropagation()}
            />
          );
        }
        return (
          <audio
            key={index}
            src={mediaUrl(att.url)}
            controls
            className={styles.attachAudio}
            onClick={(e) => e.stopPropagation()}
          />
        );
      })}
      <ImageLightbox
        src={
          lightboxIndex === null
            ? null
            : (mediaUrl(images[lightboxIndex]?.url) ?? images[lightboxIndex]?.url ?? null)
        }
        sensitive={lightboxIndex === null ? false : images[lightboxIndex]?.isSensitive}
        onClose={() => setLightboxIndex(null)}
        onPrevious={
          lightboxIndex !== null && lightboxIndex > 0
            ? () => setLightboxIndex(lightboxIndex - 1)
            : undefined
        }
        onNext={
          lightboxIndex !== null && lightboxIndex < images.length - 1
            ? () => setLightboxIndex(lightboxIndex + 1)
            : undefined
        }
      />
    </div>
  );
}

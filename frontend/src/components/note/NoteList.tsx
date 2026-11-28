import { memo, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Note } from "../../api/client";
import { useInfiniteScrollSentinel } from "../../hooks/useInfiniteScrollSentinel";
import { groupTimeline } from "../../lib/timelineGroups";
import CardStack from "../common/CardStack";
import panel from "../common/Panel.module.css";
import styles from "./NoteList.module.css";
import NoteCard from "./NoteCard";

interface NoteListProps {
  notes: Note[];
  loading?: boolean;
  emptyMessage?: string;
  linkToDetail?: boolean;
  /** リアルタイム挿入されたばかりのノート ID。押し出しアニメーションを付与する（#37）。 */
  enteringIds?: Set<string>;
  /** このノートIDの直前に「取りこぼし区間」の区切り（二重波線）を表示する。 */
  gapBeforeIds?: Set<string>;
  /** 末尾の sentinel が画面内に入ったら過去分を追加取得する（無限スクロール）。未指定なら無効。 */
  onLoadMore?: () => void;
  hasMore?: boolean;
  loadingMore?: boolean;
}

function NoteList({
  notes,
  loading,
  emptyMessage,
  linkToDetail = true,
  enteringIds,
  gapBeforeIds,
  onLoadMore,
  hasMore,
  loadingMore,
}: NoteListProps) {
  const { t } = useTranslation();
  const resolvedEmptyMessage = emptyMessage ?? t("home:noteList.emptyDefault");
  const sentinelRef = useInfiniteScrollSentinel<HTMLDivElement>(onLoadMore, hasMore);
  const groups = useMemo(() => groupTimeline(notes, gapBeforeIds), [notes, gapBeforeIds]);

  if (loading) return <p className={panel.message}>{t("common:loading")}</p>;
  if (notes.length === 0) return <p className={panel.message}>{resolvedEmptyMessage}</p>;
  return (
    <div>
      {groups.map(({ notes: group }) => {
        const first = group[0];
        return (
          // 新着でまとまりの上にリポストが足されても組み直さないよう、一番古いポストをキーにする。
          <div key={group[group.length - 1].id}>
            {gapBeforeIds?.has(first.id) && (
              <div className={styles.gap} role="separator" aria-label={t("home:noteList.gapLabel")} />
            )}
            {/* data-timeline-id はスクロール位置をポスト基準で覚えるため（lib/timelineScroll.ts）。
                区切りの下のカード自体に付け、区切りが後から挟まってもカードの位置で合わせる。
                重ねたまとまりはまとまり全体に一番上のポストのIDで付ける（中のカードは描画順が
                一覧の順と違い、文書順に縦に並ぶという前提を崩すため）。 */}
            {group.length === 1 ? (
              <div className={enteringIds?.has(first.id) ? styles.entering : undefined} data-timeline-id={first.id}>
                <NoteCard note={first} linkToDetail={linkToDetail} />
              </div>
            ) : (
              <CardStack
                className={styles.stack}
                keys={group.map((n) => n.id)}
                containerProps={{ "data-timeline-id": first.id }}
                renderCard={(i) => (
                  <div className={[styles.stackSlot, enteringIds?.has(group[i].id) && styles.entering].filter(Boolean).join(" ")}>
                    <NoteCard note={group[i]} linkToDetail={linkToDetail} stacked />
                  </div>
                )}
              />
            )}
          </div>
        );
      })}
      {onLoadMore && hasMore && (
        <div ref={sentinelRef} className={styles.sentinel}>
          {loadingMore ? t("common:loading") : ""}
        </div>
      )}
    </div>
  );
}

// 親（ホームのタイムライン等）はスワイプの状態・隣のタブの下描き等で頻繁に描き直される。
// 一覧の中身が変わっていなければ、ノート全件の描き直しを省く（指への追従が止まるため）。
export default memo(NoteList);

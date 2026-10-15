import { useTranslation } from "react-i18next";
import FilterField, { FilterBar, FilterCheckList } from "../common/FilterField";

/** 絞り込める種別。「あなた」は閲覧者本人（未ログインなら該当なし）。 */
export const PROFILE_FEED_KINDS = [
  "reaction",
  "normal",
  "reply",
  "reply_to_me",
  "quote",
  "quote_of_mine",
  "repost",
  "repost_of_mine",
] as const;

export const PROFILE_FEED_MEDIA = ["none", "image", "video", "audio"] as const;

export interface ProfileFeedFilterState {
  kinds: string[];
  media: string[];
}

export const EMPTY_PROFILE_FEED_FILTER: ProfileFeedFilterState = { kinds: [], media: [] };

/** API へ渡す絞り込み条件。種別は全部選んだ場合も未指定と同じ（`undefined`）。
 * メディアは複数指定が「すべて添付されている」の意味になるので、そのまま渡す。 */
export function toProfileFeedQuery(filter: ProfileFeedFilterState): {
  kinds?: string[];
  media?: string[];
} {
  const allKinds = filter.kinds.length === 0 || filter.kinds.length >= PROFILE_FEED_KINDS.length;
  return {
    kinds: allKinds ? undefined : filter.kinds,
    media: filter.media.length > 0 ? filter.media : undefined,
  };
}

interface ProfileFeedFilterBarProps {
  filter: ProfileFeedFilterState;
  onChange: (filter: ProfileFeedFilterState) => void;
}

/** プロフィールのアクティビティタブ最上部の絞り込み欄（種別・メディア添付）。通知の絞り込み欄と
 * 同じ見た目で、両方未指定が初期状態。2つの条件は AND。メディア添付は投稿にだけ作用し、
 * リアクションの表示には影響しない。 */
export default function ProfileFeedFilterBar({ filter, onChange }: ProfileFeedFilterBarProps) {
  const { t } = useTranslation();
  const kindLabel = (kind: string) => t(`profile:feedFilter.kinds.${kind}`);
  const mediaLabel = (media: string) => t(`profile:feedFilter.media.${media}`);
  const toggle = (list: string[], value: string) =>
    list.includes(value) ? list.filter((x) => x !== value) : [...list, value];

  const noneChecked = filter.media.includes("none");
  const otherChecked = filter.media.some((m) => m !== "none");

  return (
    <FilterBar>
      <FilterField
        label={t("profile:feedFilter.kindLabel")}
        placeholder={t("profile:feedFilter.kindPlaceholder")}
        tags={filter.kinds.map(kindLabel)}
        onClear={() => onChange({ ...filter, kinds: [] })}
      >
        <FilterCheckList
          options={PROFILE_FEED_KINDS.map((kind) => ({ value: kind, label: kindLabel(kind) }))}
          selected={filter.kinds}
          onToggle={(kind) => onChange({ ...filter, kinds: toggle(filter.kinds, kind) })}
        />
      </FilterField>
      <FilterField
        label={t("profile:feedFilter.mediaLabel")}
        placeholder={t("profile:feedFilter.mediaPlaceholder")}
        tags={filter.media.map(mediaLabel)}
        onClear={() => onChange({ ...filter, media: [] })}
      >
        {/* 「なし」と他は同時に成り立たないので、片方を選んでいる間はもう片方を選べなくする。
            画像と動画など「あり」同士は、両方が添付された投稿だけに絞る意味になる。 */}
        <FilterCheckList
          options={PROFILE_FEED_MEDIA.map((media) => ({
            value: media,
            label: mediaLabel(media),
            disabled: media === "none" ? otherChecked : noneChecked,
          }))}
          selected={filter.media}
          onToggle={(media) => onChange({ ...filter, media: toggle(filter.media, media) })}
        />
      </FilterField>
    </FilterBar>
  );
}

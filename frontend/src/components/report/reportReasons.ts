import type { TFunction } from "i18next";

// Bluesky公式（tools.ozone.report.defs）準拠の通報理由。
// 表示文言は admin.json に置き、値はOzoneのトークン名をそのまま使う。
export interface ReportCategory {
  key: string;
  options: string[];
}

export const REPORT_CATEGORIES: ReportCategory[] = [
  {
    key: "misleading",
    options: [
      "reasonMisleadingSpam",
      "reasonMisleadingScam",
      "reasonMisleadingBot",
      "reasonMisleadingImpersonation",
      "reasonMisleadingElections",
      "reasonMisleadingOther",
    ],
  },
  {
    key: "sexualAdultContent",
    options: [
      "reasonSexualUnlabeled",
      "reasonSexualAbuseContent",
      "reasonSexualNCII",
      "reasonSexualDeepfake",
      "reasonSexualAnimal",
      "reasonSexualOther",
    ],
  },
  {
    key: "harassmentHate",
    options: [
      "reasonHarassmentTroll",
      "reasonHarassmentTargeted",
      "reasonHarassmentHateSpeech",
      "reasonHarassmentDoxxing",
      "reasonHarassmentOther",
    ],
  },
  {
    key: "violencePhysicalHarm",
    options: [
      "reasonViolenceAnimal",
      "reasonViolenceThreats",
      "reasonViolenceGraphicContent",
      "reasonViolenceGlorification",
      "reasonViolenceExtremistContent",
      "reasonViolenceTrafficking",
      "reasonViolenceOther",
    ],
  },
  {
    key: "childSafety",
    options: [
      "reasonChildSafetyCSAM",
      "reasonChildSafetyGroom",
      "reasonChildSafetyPrivacy",
      "reasonChildSafetyHarassment",
      "reasonChildSafetyOther",
    ],
  },
  {
    key: "selfHarm",
    options: [
      "reasonSelfHarmContent",
      "reasonSelfHarmED",
      "reasonSelfHarmStunts",
      "reasonSelfHarmSubstances",
      "reasonSelfHarmOther",
    ],
  },
  {
    key: "ruleBreaking",
    options: [
      "reasonRuleSiteSecurity",
      "reasonRuleProhibitedSales",
      "reasonRuleBanEvasion",
      "reasonRuleOther",
    ],
  },
  { key: "other", options: ["reasonOther"] },
];

export function findReportReasonLabel(value: string, t: TFunction): string {
  const category = REPORT_CATEGORIES.find((item) =>
    item.options.includes(value),
  );
  if (!category) return value;
  return `${t(`admin:reports.categories.${category.key}.title`)} / ${t(`admin:reports.reasons.${value}`)}`;
}

// Bluesky公式（tools.ozone.report.defs）準拠の通報理由。
// カテゴリ（第1段階）と理由（第2段階）の2階層で、値はOzoneのトークン名をそのまま使う。
export interface ReportReasonOption {
  value: string;
  label: string;
}

export interface ReportCategory {
  key: string;
  title: string;
  description: string;
  options: ReportReasonOption[];
}

export const REPORT_CATEGORIES: ReportCategory[] = [
  {
    key: "misleading",
    title: "誤解を招くこと",
    description: "スパムやその他の不正行為・欺瞞",
    options: [
      { value: "reasonMisleadingSpam", label: "スパム" },
      { value: "reasonMisleadingScam", label: "詐欺" },
      { value: "reasonMisleadingBot", label: "偽のアカウントまたはボット" },
      { value: "reasonMisleadingImpersonation", label: "なりすまし" },
      { value: "reasonMisleadingElections", label: "選挙に関する誤情報" },
      { value: "reasonMisleadingOther", label: "その他の誤解を招く内容" },
    ],
  },
  {
    key: "sexualAdultContent",
    title: "成人向けコンテンツ",
    description: "ラベルのない、虐待的、または非同意の成人向けコンテンツ",
    options: [
      { value: "reasonSexualUnlabeled", label: "ラベルのない成人向けコンテンツ" },
      { value: "reasonSexualAbuseContent", label: "性的虐待コンテンツ" },
      { value: "reasonSexualNCII", label: "非同意の性的画像" },
      { value: "reasonSexualDeepfake", label: "ディープフェイクの成人向けコンテンツ" },
      { value: "reasonSexualAnimal", label: "動物性虐待" },
      { value: "reasonSexualOther", label: "その他の性的暴力コンテンツ" },
    ],
  },
  {
    key: "harassmentHate",
    title: "嫌がらせまたはヘイト",
    description: "虐待的または差別的な行為",
    options: [
      { value: "reasonHarassmentTroll", label: "荒らし" },
      { value: "reasonHarassmentTargeted", label: "特定個人への嫌がらせ" },
      { value: "reasonHarassmentHateSpeech", label: "ヘイトスピーチ" },
      { value: "reasonHarassmentDoxxing", label: "個人情報の暴露（ドキシング）" },
      { value: "reasonHarassmentOther", label: "その他の嫌がらせ・ヘイトコンテンツ" },
    ],
  },
  {
    key: "violencePhysicalHarm",
    title: "暴力",
    description: "暴力的または脅迫的なコンテンツ",
    options: [
      { value: "reasonViolenceAnimal", label: "動物福祉違反" },
      { value: "reasonViolenceThreats", label: "脅迫・扇動" },
      { value: "reasonViolenceGraphicContent", label: "グラフィックな暴力表現" },
      { value: "reasonViolenceGlorification", label: "暴力の美化" },
      { value: "reasonViolenceExtremistContent", label: "過激主義コンテンツ" },
      { value: "reasonViolenceTrafficking", label: "人身売買" },
      { value: "reasonViolenceOther", label: "その他の暴力的コンテンツ" },
    ],
  },
  {
    key: "childSafety",
    title: "児童の安全",
    description: "未成年者への加害・危険行為",
    options: [
      { value: "reasonChildSafetyCSAM", label: "児童性的虐待素材（CSAM）" },
      { value: "reasonChildSafetyGroom", label: "グルーミング・略奪的行為" },
      { value: "reasonChildSafetyPrivacy", label: "未成年者のプライバシー侵害" },
      { value: "reasonChildSafetyHarassment", label: "未成年者への嫌がらせ・いじめ" },
      { value: "reasonChildSafetyOther", label: "その他の児童安全問題" },
    ],
  },
  {
    key: "selfHarm",
    title: "自傷・危険行動",
    description: "有害または危険性の高い行為",
    options: [
      { value: "reasonSelfHarmContent", label: "自傷行為を助長・描写するコンテンツ" },
      { value: "reasonSelfHarmED", label: "摂食障害" },
      { value: "reasonSelfHarmStunts", label: "危険な挑戦・行為" },
      { value: "reasonSelfHarmSubstances", label: "危険な薬物・薬物乱用" },
      { value: "reasonSelfHarmOther", label: "その他の危険なコンテンツ" },
    ],
  },
  {
    key: "ruleBreaking",
    title: "サイトルール違反",
    description: "禁止行為・セキュリティ違反",
    options: [
      { value: "reasonRuleSiteSecurity", label: "ハッキング・システム攻撃" },
      { value: "reasonRuleProhibitedSales", label: "禁止された商品・サービスの宣伝/販売" },
      { value: "reasonRuleBanEvasion", label: "凍結逃れ（再登録）" },
      { value: "reasonRuleOther", label: "その他のルール違反" },
    ],
  },
  {
    key: "other",
    title: "その他",
    description: "上記のいずれにも当てはまらない問題",
    options: [{ value: "reasonOther", label: "その他" }],
  },
];

export function findReportReasonLabel(value: string): string {
  for (const category of REPORT_CATEGORIES) {
    const option = category.options.find((o) => o.value === value);
    if (option) return `${category.title} / ${option.label}`;
  }
  return value;
}

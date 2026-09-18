import { ReactNode, useEffect, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useSiteMeta } from "../../contexts/SiteMetaContext";
import ServerInfoDialog from "../../components/common/ServerInfoDialog";
import FitText from "./FitText";
import LoginPanel from "./LoginPanel";
import RegisterPanel, { REGISTER_PANEL_INITIAL_STATE, RegisterPanelState } from "./RegisterPanel";
import MigratePanel, { MIGRATE_PANEL_INITIAL_STATE, MigratePanelState } from "./MigratePanel";
import styles from "./AuthCarousel.module.css";

type PanelKey = "login" | "register" | "migrate";

function panelFromPath(pathname: string): PanelKey {
  if (pathname.startsWith("/register/migrate")) return "migrate";
  if (pathname.startsWith("/register")) return "register";
  return "login";
}

// サイトタイトルエリアが900px幅になる「広いモード」（issue #243の文字サイズ上限144px）。
// 880px〜1339pxはタイトルエリアが440pxへ即座に縮む「狭いモード」（上限72px）で、縦積み
// （<880px）も同じ狭いモードの文字サイズを使う。
const WIDE_BREAKPOINT = "(min-width: 1340px)";

function useIsWide() {
  const [isWide, setIsWide] = useState(() =>
    typeof window !== "undefined" ? window.matchMedia(WIDE_BREAKPOINT).matches : true
  );
  useEffect(() => {
    const mql = window.matchMedia(WIDE_BREAKPOINT);
    const onChange = () => setIsWide(mql.matches);
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, []);
  return isWide;
}

interface PanelSectionProps {
  active: boolean;
  label: string;
  to: string;
  children: ReactNode;
}

/**
 * カルーセルの1パネル。非アクティブ時は見出しボタン（他パネルへの`<Link>`）だけの高さに
 * 縮み、アクティブ時はフォーム全体を展開する。
 *
 * 非アクティブなパネルの中身（`children`）は実際にアンマウントする。`aria-hidden`/`inert`/
 * `display:none`だけで常時マウントしたまま隠す実装も試したが、Playwrightの`getByLabel`等の
 * ロケーター解決はDOM上の存在だけで判定し可視性・`inert`状態を考慮しないため、同じラベル
 * 文言を持つ他パネルのフィールド（例:「パスワード」）と衝突して`strict mode violation`に
 * なることをE2Eで確認した（実際のスクリーンリーダー利用でも紛らわしい重複を避けられる）。
 * そのため「サインアップのメール確認送信済み」等の途中状態は、このパネル自身のstateでは
 * なく親`AuthCarouselPage`側に保持し、アンマウント/再マウントをまたいで復元する。
 *
 * 非アクティブに切り替わった瞬間に即アンマウントすると、そのパネルの中身が一瞬で消えて
 * から新しいパネルが伸びる、という2段階に見える動きになってしまう（マイケル指摘）。
 * 縮小中のパネルと拡大中のパネルが同じタイミングでシンクロして動くよう、非アクティブに
 * なった後もCSSの`max-height`トランジションが終わるまで中身を残し、トランジション完了時
 * （`onTransitionEnd`）に初めてアンマウントする。
 */
function PanelSection({ active, label, to, children }: PanelSectionProps) {
  const [mounted, setMounted] = useState(active);
  useEffect(() => {
    if (active) setMounted(true);
  }, [active]);

  return (
    <section className={`${styles.panel} ${active ? styles.panelActive : ""}`}>
      {active ? (
        <h2 className={styles.panelHeading}>{label}</h2>
      ) : (
        <Link to={to} className={styles.panelHeaderButton}>
          {label}
        </Link>
      )}
      <div
        className={styles.panelBody}
        aria-hidden={!active}
        onTransitionEnd={(e) => {
          if (e.propertyName === "max-height" && !active) setMounted(false);
        }}
      >
        <div className={styles.panelBodyInner}>{mounted ? children : null}</div>
      </div>
    </section>
  );
}

/**
 * `/login`・`/register`・`/register/migrate` 共通のログイン画面本体（issue #243）。
 * ログイン・サインアップ・Blueskyから転入の3フォームを縦カルーセルとして並べ、
 * サイトタイトル・説明文（管理画面 #30/#243 設定）を横に表示する。
 *
 * この3ルートは同一の`<Route element={<AuthCarouselPage/>}>`を指すためReact Routerの
 * 差分適用でコンポーネントインスタンスが維持される（route paramを含まずJSX上も同型なため）。
 * そのため`registerState`/`migrateState`はルート間を行き来してもリセットされない。
 */
export default function AuthCarouselPage() {
  const { t } = useTranslation();
  const location = useLocation();
  const active = panelFromPath(location.pathname);
  const isWide = useIsWide();
  const { titleHtml, descriptionHtml, loginBackgroundUrl, loginBackgroundType } = useSiteMeta();
  const [serverInfoOpen, setServerInfoOpen] = useState(false);

  const [registerState, setRegisterState] = useState<RegisterPanelState>(REGISTER_PANEL_INITIAL_STATE);
  const [migrateState, setMigrateState] = useState<MigratePanelState>(MIGRATE_PANEL_INITIAL_STATE);

  function patchRegister(patch: Partial<RegisterPanelState>) {
    setRegisterState((prev) => ({ ...prev, ...patch }));
  }
  function patchMigrate(patch: Partial<MigratePanelState>) {
    setMigrateState((prev) => ({ ...prev, ...patch }));
  }

  const pageStyle =
    loginBackgroundUrl && loginBackgroundType === "image"
      ? { backgroundImage: `url(${loginBackgroundUrl})` }
      : undefined;

  return (
    <div className={styles.page} style={pageStyle}>
      {loginBackgroundUrl && loginBackgroundType === "video" && (
        <video
          className={styles.backgroundVideo}
          src={loginBackgroundUrl}
          autoPlay
          loop
          muted
          playsInline
        />
      )}

      <div className={styles.content}>
        <div className={styles.titleArea}>
          <div className={styles.titleHalf}>
            <FitText html={titleHtml} maxPx={isWide ? 144 : 72} className={styles.siteTitle} />
          </div>
          <div className={styles.descHalf}>
            <div className={styles.siteDescription} dangerouslySetInnerHTML={{ __html: descriptionHtml }} />
          </div>
        </div>

        <div className={styles.formArea}>
          <PanelSection active={active === "login"} label={t("auth:carousel.loginLabel")} to="/login">
            <LoginPanel />
          </PanelSection>
          <PanelSection active={active === "register"} label={t("auth:carousel.registerLabel")} to="/register">
            <RegisterPanel state={registerState} onChange={patchRegister} />
          </PanelSection>
          <PanelSection active={active === "migrate"} label={t("auth:carousel.migrateLabel")} to="/register/migrate">
            <MigratePanel state={migrateState} onChange={patchMigrate} />
          </PanelSection>

          <button type="button" className={styles.poweredBy} onClick={() => setServerInfoOpen(true)}>
            {t("nav:leftNav.poweredBy")}
          </button>
        </div>
      </div>

      <ServerInfoDialog open={serverInfoOpen} onClose={() => setServerInfoOpen(false)} />
    </div>
  );
}

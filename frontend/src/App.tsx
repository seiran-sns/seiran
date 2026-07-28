import { lazy, Suspense, useEffect, useState } from "react";
import { Navigate, Route, Routes, useLocation, useParams, useSearchParams } from "react-router-dom";
import { api } from "./api/client";
import { AuthProvider, useAuth } from "./contexts/AuthContext";
import { NavigationHistoryProvider } from "./contexts/NavigationHistoryContext";
import { RightPaneProvider } from "./contexts/RightPaneContext";
import { HomeFeedProvider } from "./contexts/HomeFeedContext";
import { ComposerProvider } from "./contexts/ComposerContext";
import { SiteMetaProvider } from "./contexts/SiteMetaContext";
import { StreamingProvider } from "./contexts/StreamingContext";
import { ThemeProvider } from "./contexts/ThemeContext";
import { ToastProvider } from "./contexts/ToastContext";
import HomePage from "./pages/HomePage";

const AccountSettingsPage = lazy(() => import("./pages/AccountSettingsPage"));
const AdminPage = lazy(() => import("./pages/AdminPage"));
const ForgotPassword = lazy(() => import("./pages/ForgotPassword"));
const HashtagPage = lazy(() => import("./pages/HashtagPage"));
const ListDetailPage = lazy(() => import("./pages/ListDetailPage"));
const ListsSettingsPage = lazy(() => import("./pages/ListsSettingsPage"));
const AppearanceSettingsPage = lazy(() => import("./pages/AppearanceSettingsPage"));
const Login = lazy(() => import("./pages/Login"));
const MessagesPage = lazy(() => import("./pages/MessagesPage"));
const MiAuthConnectPage = lazy(() => import("./pages/MiAuthConnectPage"));
const MutesBlocksSettingsPage = lazy(() => import("./pages/MutesBlocksSettingsPage"));
const NoteDetailPage = lazy(() => import("./pages/NoteDetailPage"));
const NotificationsPage = lazy(() => import("./pages/NotificationsPage"));
const ProfilePage = lazy(() => import("./pages/ProfilePage"));
const ProfileEditPage = lazy(() => import("./pages/ProfileEditPage"));
const Register = lazy(() => import("./pages/Register"));
const ResetPassword = lazy(() => import("./pages/ResetPassword"));
const SearchPage = lazy(() => import("./pages/SearchPage"));
const SettingsMenuPage = lazy(() => import("./pages/SettingsMenuPage"));
const AppTokensSettingsPage = lazy(() => import("./pages/AppTokensSettingsPage"));
const Setup = lazy(() => import("./pages/Setup"));
const VerifyEmail = lazy(() => import("./pages/VerifyEmail"));
const VerifyEmailChange = lazy(() => import("./pages/VerifyEmailChange"));
const TotpDisable = lazy(() => import("./pages/TotpDisable"));

function RequireAuth({ children }: { children: React.ReactNode }) {
  const { user, loading } = useAuth();
  const location = useLocation();
  if (loading) return null;
  if (!user) {
    const redirect = encodeURIComponent(location.pathname + location.search);
    return <Navigate to={`/login?redirect=${redirect}`} replace />;
  }
  return <>{children}</>;
}

function RedirectIfAuthed({ children }: { children: React.ReactNode }) {
  const { user, loading } = useAuth();
  const [searchParams] = useSearchParams();
  if (loading) return null;
  if (user) {
    const redirectTo = searchParams.get("redirect");
    return <Navigate to={redirectTo && redirectTo.startsWith("/") ? redirectTo : "/"} replace />;
  }
  return <>{children}</>;
}

/** `/@handle` 形式の permalink（#36）。`@` 始まりのときのみプロフィールを表示。 */
function ProfileByAcct() {
  const { acct } = useParams<{ acct: string }>();
  if (!acct || !acct.startsWith("@")) return <Navigate to="/" replace />;
  return <ProfilePage />;
}

function AppRoutes() {
  const [initialized, setInitialized] = useState<boolean | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    api.setup.status(controller.signal)
      .then(({ initialized }) => setInitialized(initialized))
      .catch(() => setInitialized(true)); // エラー時は初期化済みとして通常フローへ
    return () => controller.abort();
  }, []);

  if (initialized === null) return null;

  if (!initialized) {
    return (
      <Suspense fallback={null}>
        <Setup onComplete={() => setInitialized(true)} />
      </Suspense>
    );
  }

  return (
    <NavigationHistoryProvider>
      <Suspense fallback={null}>
        <Routes>
        <Route
          path="/"
          element={
            <RequireAuth>
              <HomePage />
            </RequireAuth>
          }
        />
        <Route
          path="/search"
          element={
            <RequireAuth>
              <SearchPage />
            </RequireAuth>
          }
        />
        <Route
          path="/notifications"
          element={
            <RequireAuth>
              <NotificationsPage />
            </RequireAuth>
          }
        />
        <Route
          path="/notes/:id"
          element={
            <RequireAuth>
              <NoteDetailPage />
            </RequireAuth>
          }
        />
        <Route
          path="/profile"
          element={
            <RequireAuth>
              <ProfilePage />
            </RequireAuth>
          }
        />
        <Route
          path="/admin"
          element={
            <RequireAuth>
              <AdminPage />
            </RequireAuth>
          }
        />
        <Route
          path="/settings"
          element={
            <RequireAuth>
              <SettingsMenuPage />
            </RequireAuth>
          }
        />
        <Route
          path="/settings/account"
          element={
            <RequireAuth>
              <AccountSettingsPage />
            </RequireAuth>
          }
        />
        <Route
          path="/settings/mutes-blocks"
          element={
            <RequireAuth>
              <MutesBlocksSettingsPage />
            </RequireAuth>
          }
        />
        <Route
          path="/settings/profile"
          element={
            <RequireAuth>
              <ProfileEditPage />
            </RequireAuth>
          }
        />
        <Route
          path="/settings/lists"
          element={
            <RequireAuth>
              <ListsSettingsPage />
            </RequireAuth>
          }
        />
        <Route
          path="/settings/appearance"
          element={
            <RequireAuth>
              <AppearanceSettingsPage />
            </RequireAuth>
          }
        />
        <Route
          path="/settings/app-tokens"
          element={
            <RequireAuth>
              <AppTokensSettingsPage />
            </RequireAuth>
          }
        />
        <Route
          path="/lists/:id"
          element={
            <RequireAuth>
              <ListDetailPage />
            </RequireAuth>
          }
        />
        <Route
          path="/tags/:name"
          element={
            <RequireAuth>
              <HashtagPage />
            </RequireAuth>
          }
        />
        <Route
          path="/messages"
          element={
            <RequireAuth>
              <MessagesPage />
            </RequireAuth>
          }
        />
        <Route
          path="/messages/:threadRootId"
          element={
            <RequireAuth>
              <MessagesPage />
            </RequireAuth>
          }
        />
        <Route
          path="/connect/:sessionId"
          element={
            <RequireAuth>
              <MiAuthConnectPage />
            </RequireAuth>
          }
        />
        <Route
          path="/:acct"
          element={
            <RequireAuth>
              <ProfileByAcct />
            </RequireAuth>
          }
        />
        <Route
          path="/login"
          element={
            <RedirectIfAuthed>
              <Login />
            </RedirectIfAuthed>
          }
        />
        <Route
          path="/register"
          element={
            <RedirectIfAuthed>
              <Register />
            </RedirectIfAuthed>
          }
        />
        <Route path="/verify-email" element={<VerifyEmail />} />
        <Route path="/verify-email-change" element={<VerifyEmailChange />} />
        <Route path="/totp-disable" element={<TotpDisable />} />
        <Route path="/forgot-password" element={<ForgotPassword />} />
        <Route path="/reset-password" element={<ResetPassword />} />
        </Routes>
      </Suspense>
    </NavigationHistoryProvider>
  );
}

export default function App() {
  return (
    <ThemeProvider>
      <SiteMetaProvider>
        <ToastProvider>
          <AuthProvider>
            <StreamingProvider>
              <RightPaneProvider>
                <HomeFeedProvider>
                  <ComposerProvider>
                    <AppRoutes />
                  </ComposerProvider>
                </HomeFeedProvider>
              </RightPaneProvider>
            </StreamingProvider>
          </AuthProvider>
        </ToastProvider>
      </SiteMetaProvider>
    </ThemeProvider>
  );
}

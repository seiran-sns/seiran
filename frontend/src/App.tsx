import { useEffect, useState } from "react";
import { Navigate, Route, Routes, useLocation, useParams, useSearchParams } from "react-router-dom";
import { api } from "./api/client";
import { AuthProvider, useAuth } from "./contexts/AuthContext";
import { NavigationHistoryProvider } from "./contexts/NavigationHistoryContext";
import { RightPaneProvider } from "./contexts/RightPaneContext";
import { HomeFeedProvider } from "./contexts/HomeFeedContext";
import { ComposerProvider } from "./contexts/ComposerContext";
import { SiteMetaProvider } from "./contexts/SiteMetaContext";
import { StreamingProvider } from "./contexts/StreamingContext";
import { ToastProvider } from "./contexts/ToastContext";
import AccountSettingsPage from "./pages/AccountSettingsPage";
import AdminPage from "./pages/AdminPage";
import ForgotPassword from "./pages/ForgotPassword";
import HashtagPage from "./pages/HashtagPage";
import HomePage from "./pages/HomePage";
import ListDetailPage from "./pages/ListDetailPage";
import ListsSettingsPage from "./pages/ListsSettingsPage";
import AppearanceSettingsPage from "./pages/AppearanceSettingsPage";
import Login from "./pages/Login";
import MessagesPage from "./pages/MessagesPage";
import MiAuthConnectPage from "./pages/MiAuthConnectPage";
import MutesBlocksSettingsPage from "./pages/MutesBlocksSettingsPage";
import NoteDetailPage from "./pages/NoteDetailPage";
import NotificationsPage from "./pages/NotificationsPage";
import ProfilePage from "./pages/ProfilePage";
import ProfileEditPage from "./pages/ProfileEditPage";
import Register from "./pages/Register";
import ResetPassword from "./pages/ResetPassword";
import SearchPage from "./pages/SearchPage";
import SettingsMenuPage from "./pages/SettingsMenuPage";
import AppTokensSettingsPage from "./pages/AppTokensSettingsPage";
import Setup from "./pages/Setup";
import VerifyEmail from "./pages/VerifyEmail";
import VerifyEmailChange from "./pages/VerifyEmailChange";

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
    return <Setup onComplete={() => setInitialized(true)} />;
  }

  return (
    <NavigationHistoryProvider>
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
        <Route path="/forgot-password" element={<ForgotPassword />} />
        <Route path="/reset-password" element={<ResetPassword />} />
      </Routes>
    </NavigationHistoryProvider>
  );
}

export default function App() {
  return (
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
  );
}

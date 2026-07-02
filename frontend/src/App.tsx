import { useEffect, useState } from "react";
import { Navigate, Route, Routes } from "react-router-dom";
import { api } from "./api/client";
import { AuthProvider, useAuth } from "./contexts/AuthContext";
import ForgotPassword from "./pages/ForgotPassword";
import Login from "./pages/Login";
import NoteDetail from "./pages/NoteDetail";
import Register from "./pages/Register";
import ResetPassword from "./pages/ResetPassword";
import Setup from "./pages/Setup";
import Timeline from "./pages/Timeline";
import UserProfilePage from "./pages/UserProfile";
import VerifyEmail from "./pages/VerifyEmail";

function RequireAuth({ children }: { children: React.ReactNode }) {
  const { user, loading } = useAuth();
  if (loading) return null;
  return user ? <>{children}</> : <Navigate to="/login" replace />;
}

function RedirectIfAuthed({ children }: { children: React.ReactNode }) {
  const { user, loading } = useAuth();
  if (loading) return null;
  return user ? <Navigate to="/" replace /> : <>{children}</>;
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
    <Routes>
      <Route
        path="/"
        element={
          <RequireAuth>
            <Timeline />
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
      <Route path="/forgot-password" element={<ForgotPassword />} />
      <Route path="/reset-password" element={<ResetPassword />} />
      <Route path="/notes/:id" element={<NoteDetail />} />
      <Route
        path="/profile"
        element={
          <RequireAuth>
            <UserProfilePage />
          </RequireAuth>
        }
      />
    </Routes>
  );
}

export default function App() {
  return (
    <AuthProvider>
      <AppRoutes />
    </AuthProvider>
  );
}

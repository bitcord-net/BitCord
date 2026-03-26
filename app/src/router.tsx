import { createBrowserRouter, Navigate } from "react-router-dom";
import { OnboardingPage } from "./pages/OnboardingPage";
import { UnlockPage } from "./pages/UnlockPage";
import { AppLayout } from "./layouts/AppLayout";
import { ProtectedRoute } from "./components/ProtectedRoute";
import { CommunitySettingsPage } from "./pages/CommunitySettingsPage";
import { ChatView } from "./pages/ChatView";
import { DMConversationView } from "./pages/DMConversationView";
import { DMListPlaceholder } from "./pages/DMListPlaceholder";
import { SettingsPage } from "./pages/SettingsPage";

export const router = createBrowserRouter([
  {
    path: "/",
    element: <Navigate to="/app" replace />,
  },
  {
    path: "/onboarding",
    element: <OnboardingPage />,
  },
  {
    path: "/unlock",
    element: <UnlockPage />,
  },
  {
    path: "/app",
    element: (
      <ProtectedRoute>
        <AppLayout />
      </ProtectedRoute>
    ),
    children: [
      {
        // Redirect bare /app to DMs
        index: true,
        element: <Navigate to="dm/" replace />,
      },
      {
        path: "community/:cid/channel/:chid",
        element: <ChatView />,
      },
      {
        path: "community/:cid/channel/",
        element: null, // No channel selected yet
      },
      {
        path: "community/:cid/settings",
        element: <CommunitySettingsPage />,
      },
      {
        path: "dm/:peerId",
        element: <DMConversationView />,
      },
      {
        path: "dm/",
        element: <DMListPlaceholder />,
      },
      {
        path: "settings",
        element: <SettingsPage />,
      },
    ],
  },
]);

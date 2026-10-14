import { createRoot } from "react-dom/client";
import "./styles/landing.css";
import { StreamProvider } from "./state/stream.tsx";
import { ToastProvider } from "./state/toasts.tsx";
import { applySavedTheme } from "./theme.ts";
import { LandingPage } from "./pages/LandingPage.tsx";

applySavedTheme();
createRoot(document.getElementById("root")!).render(
  <StreamProvider>
    <ToastProvider>
      <LandingPage />
    </ToastProvider>
  </StreamProvider>,
);

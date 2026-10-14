import { createRoot } from "react-dom/client";
import "./styles/app.css";
import { StreamProvider } from "./state/stream.tsx";
import { ToastProvider } from "./state/toasts.tsx";
import { applySavedTheme } from "./theme.ts";
import { MarketsPage } from "./pages/MarketsPage.tsx";

applySavedTheme();
createRoot(document.getElementById("root")!).render(
  <StreamProvider>
    <ToastProvider>
      <MarketsPage />
    </ToastProvider>
  </StreamProvider>,
);

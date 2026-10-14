// Short pop-up messages in the corner. Each one removes itself after a few seconds.
import { createContext, useCallback, useContext, useState, type ReactNode } from "react";

export type ToastKind = "busy" | "good" | "error";
interface Toast { id: number; title: string; text: string; kind: ToastKind }

interface ToastApi { push: (title: string, text: string, kind: ToastKind) => void }
const ToastContext = createContext<ToastApi>({ push: () => {} });

let nextId = 1;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);

  const push = useCallback((title: string, text: string, kind: ToastKind) => {
    const id = nextId++;
    setToasts((list) => [...list.slice(-3), { id, title, text, kind }]);
    setTimeout(() => setToasts((list) => list.filter((t) => t.id !== id)), kind === "busy" ? 4000 : 6000);
  }, []);

  return (
    <ToastContext.Provider value={{ push }}>
      {children}
      <div className="toasts" aria-live="polite">
        {toasts.map((t) => (
          <div key={t.id} className={`toast ${t.kind}`}>
            <b>{t.title}</b>
            <span>{t.text}</span>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export function useToast(): ToastApi {
  return useContext(ToastContext);
}

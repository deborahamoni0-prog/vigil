// Switches between dark and light, and remembers the choice in this browser.
import { useState } from "react";

function currentTheme(): "dark" | "light" {
  const set = document.documentElement.dataset.theme;
  if (set === "dark" || set === "light") return set;
  return matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

export function ThemeButton() {
  const [theme, setTheme] = useState(currentTheme);
  const toggle = () => {
    const next = theme === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    try {
      localStorage.setItem("vigil-theme", next);
    } catch {
      // Not saved in private mode. The choice still applies for this visit.
    }
    setTheme(next);
  };
  return (
    <button className="icon-btn" onClick={toggle} aria-label="Switch theme" title="Switch light and dark">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
        <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
      </svg>
    </button>
  );
}

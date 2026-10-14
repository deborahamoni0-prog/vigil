// Applies the saved light or dark choice before the first paint.
export function applySavedTheme(): void {
  try {
    const saved = localStorage.getItem("vigil-theme");
    if (saved === "dark" || saved === "light") document.documentElement.dataset.theme = saved;
  } catch {
    // Storage can be blocked. The system setting is used then.
  }
}

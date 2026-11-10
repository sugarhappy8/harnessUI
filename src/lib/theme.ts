export type ThemeChoice = "system" | "light" | "dark";

const STORAGE_KEY = "theme";

export function loadTheme(): ThemeChoice {
  try {
    const value = window.localStorage.getItem(STORAGE_KEY);
    if (value === "light" || value === "dark" || value === "system") return value;
  } catch {
    // Storage can be unavailable; fall back to the OS setting.
  }
  return "system";
}

export function applyTheme(choice: ThemeChoice): void {
  const root = document.documentElement;
  if (choice === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", choice);
  try {
    window.localStorage.setItem(STORAGE_KEY, choice);
  } catch {
    // Not persisted; the choice still applies for this session.
  }
}

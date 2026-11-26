// Light and dark are one set of tokens switched by `data-theme` on <html>.
// The choice is kept in localStorage when it is available, and follows the
// operating system until a person makes one.
export type Theme = 'light' | 'dark';
const KEY = 'vkore.theme';

export function preferredTheme(): Theme {
  try {
    const saved = localStorage.getItem(KEY);
    if (saved === 'light' || saved === 'dark') return saved;
  } catch {
    // Storage can be unavailable; the OS preference below still applies.
  }
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
}

export function saveTheme(theme: Theme): void {
  try {
    localStorage.setItem(KEY, theme);
  } catch {
    // Not persisted; still applied for this session.
  }
  applyTheme(theme);
}

// Terminal input from a key press. The guest's shell reads bytes, so each key
// becomes the bytes a terminal would send; a key that is not terminal input
// (a browser or window shortcut) is `null` and left alone.

export interface KeyLike {
  key: string;
  ctrlKey: boolean;
  altKey: boolean;
  metaKey: boolean;
  shiftKey?: boolean;
}

const ARROWS: Record<string, string> = { ArrowUp: 'A', ArrowDown: 'B', ArrowRight: 'C', ArrowLeft: 'D' };
const FIXED: Record<string, string> = {
  Enter: '\r',
  Backspace: '\x7f',
  Tab: '\t',
  Escape: '\x1b',
  Home: '\x1b[H',
  End: '\x1b[F',
  Delete: '\x1b[3~',
  PageUp: '\x1b[5~',
  PageDown: '\x1b[6~',
};

export function keyToInput(event: KeyLike, applicationCursor: boolean): string | null {
  // Command-key shortcuts (copy, paste, reload) belong to the window.
  if (event.metaKey) return null;
  // Shift+Tab is the way out: a console that swallowed every key would trap keyboard users.
  if (event.key === 'Tab' && event.shiftKey) return null;
  const arrow = ARROWS[event.key];
  if (arrow) return `\x1b${applicationCursor ? 'O' : '['}${arrow}`;
  const fixed = FIXED[event.key];
  if (fixed) return event.altKey ? `\x1b${fixed}` : fixed;
  // One character: a letter, digit, symbol or a single emoji.
  if ([...event.key].length !== 1) return null;
  if (event.ctrlKey) {
    const code = event.key.toLowerCase().charCodeAt(0);
    if (code >= 0x61 && code <= 0x7a) return String.fromCharCode(code & 0x1f);
    if (event.key === ' ') return '\x00';
    if (event.key === '[') return '\x1b';
    if (event.key === '\\') return '\x1c';
    if (event.key === ']') return '\x1d';
    return null;
  }
  return event.altKey ? `\x1b${event.key}` : event.key;
}

/** A terminal colour as CSS, or `null` for the terminal's own default. */
export function paintCss(paint: null | number | string): string | null {
  if (paint === null) return null;
  if (typeof paint === 'string') return /^#[0-9a-f]{6}$/i.test(paint) ? paint : null;
  // The sixteen named colours come from the theme (`--ansi-N`), so they read on a light
  // terminal as well as a dark one.
  if (paint < 16) return `var(--ansi-${paint})`;
  if (paint < 232) {
    const n = paint - 16;
    const level = (v: number) => (v === 0 ? 0 : 55 + v * 40);
    return `rgb(${level(Math.floor(n / 36))}, ${level(Math.floor((n % 36) / 6))}, ${level(n % 6)})`;
  }
  if (paint < 256) {
    const gray = 8 + (paint - 232) * 10;
    return `rgb(${gray}, ${gray}, ${gray})`;
  }
  return null;
}

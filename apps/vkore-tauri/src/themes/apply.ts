import { contrast, mix, readable } from './colour';
import { DEFAULT_ID, presetById, type Preset } from './presets';

export type Mode = 'light' | 'dark';

const WHITE = '#ffffff';
const BLACK = '#000000';
const KEY = 'vkore.preset';

/** The CSS variables a preset sets for `mode`. Status colours stay the stylesheet's. */
export function tokens(preset: Preset, mode: Mode): Record<string, string> {
  const dark = mode === 'dark';
  const bg = dark ? preset.bg : mix(preset.panel, WHITE, 0.55);
  const surface = dark ? mix(bg, preset.panel, 0.06) : mix(preset.panel, WHITE, 0.8);
  const surface2 = dark ? mix(bg, preset.panel, 0.11) : mix(bg, preset.body, 0.06);
  const border = dark ? mix(bg, preset.panel, 0.2) : mix(bg, preset.body, 0.16);
  const text = dark ? preset.panel : preset.body;
  // Muted and faint text is mixed towards the background, then pushed back until it reads.
  const muted = readable(mix(text, bg, 0.25), text, surface, 4.5);
  const faint = readable(mix(text, bg, 0.45), text, surface, 3.2);
  const accent = preset.accent;
  const accentText = readable(mix(accent, text, 0.2), text, surface, 4.5);
  const onAccent = contrast(BLACK, accent) > contrast(WHITE, accent) ? BLACK : WHITE;
  const soft = mix(dark ? bg : surface, accent, dark ? 0.2 : 0.16);
  return {
    '--bg': bg,
    '--surface': surface,
    '--surface-2': surface2,
    '--border': border,
    '--text': text,
    '--ink': text,
    '--muted': muted,
    '--faint': faint,
    '--accent': accent,
    '--accent-soft': soft,
    '--accent-text': accentText,
    '--warm': accentText,
    '--warm-soft': soft,
    '--coral-d': accent,
    '--coral': mix(accent, WHITE, 0.18),
    '--on-accent': onAccent,
    '--mp-body': preset.body,
    '--mp-panel': preset.panel,
    '--mp-side': preset.panelSide,
    '--mp-accent': accent,
    '--mp-screen': preset.screen,
    '--mp-edge-hi': preset.edgeHi,
    '--mp-edge-dark': preset.edgeDark,
  };
}

const ALL = Object.keys(tokens(presetById(DEFAULT_ID), 'light'));

/** Apply a preset to the page. The default clears every override, leaving the stylesheet's own theme. */
export function applyPreset(id: string, mode: Mode, root: HTMLElement = document.documentElement): void {
  root.dataset.preset = id;
  if (id === DEFAULT_ID) {
    for (const name of ALL) root.style.removeProperty(name);
    return;
  }
  for (const [name, value] of Object.entries(tokens(presetById(id), mode))) {
    root.style.setProperty(name, value);
  }
}

export function preferredPreset(): string {
  try {
    const saved = localStorage.getItem(KEY);
    if (saved) return presetById(saved).id;
  } catch {
    // Storage can be unavailable; the default applies.
  }
  return DEFAULT_ID;
}

export function savePreset(id: string): void {
  try {
    localStorage.setItem(KEY, id);
  } catch {
    // Not persisted; still applied for this session.
  }
}

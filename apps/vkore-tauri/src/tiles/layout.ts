// Saving and restoring a machine's tile layout. A layout is dockview's own JSON;
// it is kept per machine in localStorage when that is available, and anything that
// does not look like a layout from this version of the app is ignored, so a stale
// or edited entry can only ever cost the default layout.
import { TILE_KINDS } from './tiles';

const VERSION = 'v1';

export function layoutKey(name: string): string {
  return `vkore.workspace.${VERSION}.${name}`;
}

/** The layout in `text`, or `null` when it is missing, not JSON, or names a tile this app does not have. */
export function parseLayout(text: string | null): unknown | null {
  if (!text || text.length > 200_000) return null;
  try {
    const data = JSON.parse(text) as { panels?: Record<string, { contentComponent?: string }>; grid?: unknown };
    if (!data || typeof data !== 'object' || !data.panels || typeof data.panels !== 'object' || !data.grid) return null;
    const known = new Set<string>(TILE_KINDS);
    for (const panel of Object.values(data.panels)) {
      if (!panel || typeof panel.contentComponent !== 'string' || !known.has(panel.contentComponent)) return null;
    }
    return data;
  } catch {
    return null;
  }
}

export function loadLayout(name: string): unknown | null {
  try {
    return parseLayout(localStorage.getItem(layoutKey(name)));
  } catch {
    return null;
  }
}

export function saveLayout(name: string, layout: unknown): void {
  try {
    localStorage.setItem(layoutKey(name), JSON.stringify(layout));
  } catch {
    // Not persisted; the layout still works for this session.
  }
}

export function forgetLayout(name: string): void {
  try {
    localStorage.removeItem(layoutKey(name));
  } catch {
    // Nothing to forget.
  }
}

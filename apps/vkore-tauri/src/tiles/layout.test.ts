import { describe, expect, it } from 'vitest';
import { layoutKey, parseLayout } from './layout';

const layout = (components: string[]) =>
  JSON.stringify({
    grid: { root: {}, height: 1, width: 1, orientation: 'HORIZONTAL' },
    panels: Object.fromEntries(components.map((c) => [c, { id: c, contentComponent: c, title: c }])),
  });

describe('tile layouts', () => {
  it('are kept per machine and per version', () => {
    expect(layoutKey('a')).not.toBe(layoutKey('b'));
    expect(layoutKey('a')).toContain('v1');
  });
  it('accept a layout of known tiles', () => {
    expect(parseLayout(layout(['console', 'events']))).not.toBeNull();
  });
  it('refuse anything else, so only the default layout is ever lost', () => {
    expect(parseLayout(null)).toBeNull();
    expect(parseLayout('')).toBeNull();
    expect(parseLayout('not json')).toBeNull();
    expect(parseLayout('{}')).toBeNull();
    expect(parseLayout(layout(['console', 'a-tile-from-another-version']))).toBeNull();
    expect(parseLayout(JSON.stringify({ grid: {}, panels: { x: { contentComponent: 5 } } }))).toBeNull();
    expect(parseLayout('x'.repeat(300_000))).toBeNull();
  });
});

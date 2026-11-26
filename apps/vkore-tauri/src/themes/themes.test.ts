import { describe, expect, it } from 'vitest';
import { contrast, mix, readable } from './colour';
import { tokens } from './apply';
import { DEFAULT_ID, PRESETS, presetById } from './presets';

const HEX = /^#[0-9a-f]{6}$/;

describe('colour', () => {
  it('mixes and measures', () => {
    expect(mix('#000000', '#ffffff', 0.5)).toBe('#808080');
    expect(contrast('#000000', '#ffffff')).toBeCloseTo(21, 0);
    expect(contrast('#777777', '#777777')).toBe(1);
  });
  it('pushes a colour until it reads', () => {
    const out = readable('#ffeecc', '#000000', '#ffffff', 4.5);
    expect(contrast(out, '#ffffff')).toBeGreaterThanOrEqual(4.5);
  });
});

describe('presets', () => {
  it('are the default plus the logo studio palettes, with unique ids', () => {
    expect(PRESETS.length).toBe(13);
    expect(new Set(PRESETS.map((p) => p.id)).size).toBe(PRESETS.length);
    expect(PRESETS[0]!.id).toBe(DEFAULT_ID);
    for (const preset of PRESETS) {
      for (const colour of [preset.body, preset.panel, preset.panelSide, preset.accent, preset.screen, preset.edgeHi, preset.edgeDark, preset.bg]) {
        expect(colour, preset.id).toMatch(HEX);
      }
    }
  });
  it('fall back to the default for an id that is not one', () => {
    expect(presetById('nonsense').id).toBe(DEFAULT_ID);
    expect(presetById(null).id).toBe(DEFAULT_ID);
  });
  it.each(PRESETS.map((p) => [p.id, p] as const))('%s keeps text readable in light and dark', (_id, preset) => {
    for (const mode of ['light', 'dark'] as const) {
      const t = tokens(preset, mode);
      for (const value of Object.values(t)) expect(value).toMatch(HEX);
      expect(contrast(t['--text']!, t['--surface']!), `${mode} text`).toBeGreaterThanOrEqual(7);
      expect(contrast(t['--muted']!, t['--surface']!), `${mode} muted`).toBeGreaterThanOrEqual(4.5);
      expect(contrast(t['--faint']!, t['--surface']!), `${mode} faint`).toBeGreaterThanOrEqual(3);
      expect(contrast(t['--accent-text']!, t['--surface']!), `${mode} accent text`).toBeGreaterThanOrEqual(4.5);
      expect(contrast(t['--on-accent']!, t['--coral-d']!), `${mode} button`).toBeGreaterThanOrEqual(3);
      expect(contrast(t['--text']!, t['--bg']!), `${mode} page`).toBeGreaterThanOrEqual(7);
    }
  });
});

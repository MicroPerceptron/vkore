// Colour presets, taken from the logo studio's palettes (`mp_color_explorer.html`).
// A preset is eight colours; `apply.ts` derives the whole interface from them for
// light and for dark, so a new preset is one entry here and nothing else.

export interface Preset {
  id: string;
  name: string;
  desc: string;
  /** The mark's dark shell. */
  body: string;
  /** The mark's light panel. */
  panel: string;
  panelSide: string;
  /** The mark's screen bars: the interface's accent. */
  accent: string;
  screen: string;
  edgeHi: string;
  edgeDark: string;
  /** The studio's page background: the interface's dark background. */
  bg: string;
}

/** The theme the interface ships with: no overrides at all. */
export const DEFAULT_ID = 'default';

export const PRESETS: readonly Preset[] = [
  {
    id: DEFAULT_ID,
    name: 'Sand & Teal',
    desc: 'The dashboard look: warm sand, deep teal, coral',
    body: '#3a3230',
    panel: '#f0e8dc',
    panelSide: '#e8e0d4',
    accent: '#2a9d8f',
    screen: '#3a3230',
    edgeHi: '#9a8e84',
    edgeDark: '#4a4240',
    bg: '#0d1412',
  },
  {
    id: 'original',
    name: 'Original',
    desc: 'Deep plum + cream + amber',
    body: '#2a0f36',
    panel: '#f2f1b1',
    panelSide: '#f3f2b1',
    accent: '#f69721',
    screen: '#2a0f36',
    edgeHi: '#a69b86',
    edgeDark: '#544453',
    bg: '#0c0612',
  },
  {
    id: 'midnight',
    name: 'Midnight Terminal',
    desc: 'Navy + gray + phosphor green',
    body: '#0a1628',
    panel: '#c8d0dc',
    panelSide: '#b8c2d0',
    accent: '#22d465',
    screen: '#0a1628',
    edgeHi: '#6a7888',
    edgeDark: '#1a2838',
    bg: '#020810',
  },
  {
    id: 'ember',
    name: 'Ember',
    desc: 'Charcoal + warm stone + coral',
    body: '#1f1a18',
    panel: '#e8ddd0',
    panelSide: '#ddd2c5',
    accent: '#e8503a',
    screen: '#1f1a18',
    edgeHi: '#9a8d80',
    edgeDark: '#3a3230',
    bg: '#0e0a08',
  },
  {
    id: 'arctic',
    name: 'Arctic',
    desc: 'Slate blue + ice white + cyan',
    body: '#1c2a3a',
    panel: '#e4eaf0',
    panelSide: '#d6dce4',
    accent: '#18b4cc',
    screen: '#1c2a3a',
    edgeHi: '#7a8a9a',
    edgeDark: '#2c3a4a',
    bg: '#080e16',
  },
  {
    id: 'solar',
    name: 'Solar Flare',
    desc: 'Dark bronze + golden + vivid orange',
    body: '#2c1a08',
    panel: '#f5e6b8',
    panelSide: '#eadcae',
    accent: '#ff8c1a',
    screen: '#2c1a08',
    edgeHi: '#9a8060',
    edgeDark: '#4a3418',
    bg: '#120c02',
  },
  {
    id: 'phantom',
    name: 'Phantom',
    desc: 'Near-black + silver + violet',
    body: '#141018',
    panel: '#c4bcc8',
    panelSide: '#b4acb8',
    accent: '#9b5de5',
    screen: '#141018',
    edgeHi: '#7a7280',
    edgeDark: '#242028',
    bg: '#08060a',
  },
  {
    id: 'forest',
    name: 'Deep Forest',
    desc: 'Dark pine + sage + warm gold',
    body: '#0f2018',
    panel: '#d4dcc8',
    panelSide: '#c8d0bc',
    accent: '#d4a026',
    screen: '#0f2018',
    edgeHi: '#7a8a70',
    edgeDark: '#1a3020',
    bg: '#040c08',
  },
  {
    id: 'blueprint',
    name: 'Blueprint',
    desc: 'Deep blue + white + signal red',
    body: '#142040',
    panel: '#e8ecf4',
    panelSide: '#dce0e8',
    accent: '#e03030',
    screen: '#142040',
    edgeHi: '#6a7a9a',
    edgeDark: '#1a2848',
    bg: '#060c18',
  },
  {
    id: 'cream',
    name: 'Cream Machine',
    desc: 'Warm taupe + off-white + teal',
    body: '#3a3230',
    panel: '#f0e8dc',
    panelSide: '#e8e0d4',
    accent: '#2a9d8f',
    screen: '#3a3230',
    edgeHi: '#9a8e84',
    edgeDark: '#4a4240',
    bg: '#1a1614',
  },
  {
    id: 'neon',
    name: 'Neon Nights',
    desc: 'Jet black + light gray + hot pink',
    body: '#10101a',
    panel: '#d0ccd8',
    panelSide: '#c0bcc8',
    accent: '#ff2d78',
    screen: '#10101a',
    edgeHi: '#6a6878',
    edgeDark: '#20202a',
    bg: '#060608',
  },
  {
    id: 'sand',
    name: 'Sandstone',
    desc: 'Terracotta + linen + burnt sienna',
    body: '#3c2420',
    panel: '#ede4d4',
    panelSide: '#e2d9c9',
    accent: '#c65d28',
    screen: '#3c2420',
    edgeHi: '#9a8878',
    edgeDark: '#4e3430',
    bg: '#160e0c',
  },
  {
    id: 'mono',
    name: 'Monochrome',
    desc: 'Pure black + white + gray',
    body: '#161616',
    panel: '#e6e6e6',
    panelSide: '#dadada',
    accent: '#787878',
    screen: '#161616',
    edgeHi: '#7a7a7a',
    edgeDark: '#2a2a2a',
    bg: '#080808',
  },
] as const;

export function presetById(id: string | null | undefined): Preset {
  return PRESETS.find((preset) => preset.id === id) ?? PRESETS[0]!;
}

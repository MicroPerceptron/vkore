// Small colour arithmetic for deriving a theme: mixing in sRGB and WCAG contrast.

export type Rgb = [number, number, number];

export function parse(hex: string): Rgb {
  const value = hex.replace('#', '');
  return [0, 2, 4].map((at) => parseInt(value.slice(at, at + 2), 16)) as Rgb;
}

export function format([r, g, b]: Rgb): string {
  return `#${[r, g, b].map((channel) => Math.round(Math.min(255, Math.max(0, channel))).toString(16).padStart(2, '0')).join('')}`;
}

/** `amount` of `b` mixed into `a`: 0 is `a`, 1 is `b`. */
export function mix(a: string, b: string, amount: number): string {
  const [ar, ag, ab] = parse(a);
  const [br, bg, bb] = parse(b);
  return format([ar + (br - ar) * amount, ag + (bg - ag) * amount, ab + (bb - ab) * amount]);
}

function channel(value: number): number {
  const unit = value / 255;
  return unit <= 0.03928 ? unit / 12.92 : ((unit + 0.055) / 1.055) ** 2.4;
}

export function luminance(hex: string): number {
  const [r, g, b] = parse(hex);
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/** WCAG contrast ratio, 1 to 21. */
export function contrast(a: string, b: string): number {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x) as [number, number];
  return (light + 0.05) / (dark + 0.05);
}

/** Move `colour` towards `target` until it reads at `ratio` against `against`. */
export function readable(colour: string, target: string, against: string, ratio: number): string {
  let out = colour;
  for (let step = 0; step <= 20 && contrast(out, against) < ratio; step += 1) {
    out = mix(colour, target, step / 20);
  }
  return out;
}

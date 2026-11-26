import { DOTS, DOT_RADIUS, PATHS, VIEWBOX } from './mark';

/** How the mark moves. `intro` plays once; `busy` loops like a bad signal; `still` does not move. */
export type MarkMotion = 'intro' | 'busy' | 'still';

// The panel's window is cut out of it (even-odd) so the screen can move on its own.
const PANEL_FRONT = `${PATHS.panel_front} ${PATHS.screen}`;

/**
 * The MicroPerceptron mark as inline SVG. Its motion is CSS keyframes ported
 * from the logo studio's CRT Boot (intro) and Signal Noise (busy) presets, so it
 * needs no animation library and nothing beyond the page's own stylesheet.
 */
export function MpMark({ motion = 'still', size = 40, title }: { motion?: MarkMotion; size?: number; title?: string }) {
  return (
    <svg
      className={`mp mp-${motion}`}
      viewBox={`0 0 ${VIEWBOX} ${VIEWBOX}`}
      width={size}
      height={size}
      role={title ? 'img' : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
    >
      <path className="mp-body" d={PATHS.body} />
      <path className="mp-panel_front" d={PANEL_FRONT} fillRule="evenodd" />
      <path className="mp-panel_side" d={PATHS.panel_side} />
      <path className="mp-edge_hi" d={PATHS.edge_hi} />
      <path className="mp-panel_detail" d={PATHS.panel_detail} />
      <path className="mp-side_edge" d={PATHS.side_edge} />
      <path className="mp-screen" d={PATHS.screen} />
      <path className="mp-bar_top" d={PATHS.bar_top} />
      <path className="mp-content_orange" d={PATHS.content_orange} />
      <path className="mp-glasses_R" d={PATHS.glasses_R} />
      <path className="mp-glasses_L" d={PATHS.glasses_L} />
      {DOTS.map((dot) => (
        <circle key={dot.id} className={`mp-${dot.id}`} cx={dot.cx} cy={dot.cy} r={DOT_RADIUS} />
      ))}
    </svg>
  );
}

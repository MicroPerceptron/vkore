import { AlertTriangle, CircleCheck, CircleDot, Loader, Moon, Power, type LucideIcon } from 'lucide-react';
import type { Machine, Phase, Stage } from './api';
import { share } from './format';

export const STAGES: { stage: Stage; label: string; hint: string }[] = [
  { stage: 'firmware', label: 'Firmware', hint: 'UEFI hands over to the loader' },
  { stage: 'kernel', label: 'Kernel', hint: 'The console is up' },
  { stage: 'services', label: 'Services', hint: 'Every service row is founded' },
  { stage: 'shell', label: 'Shell', hint: 'The shell answers' },
  { stage: 'ready', label: 'Ready', hint: 'Applications are seeded' },
];

const PHASES: Record<Phase, { label: string; tone: string; Icon: LucideIcon }> = {
  idle: { label: 'Stopped', tone: 'quiet', Icon: Moon },
  starting: { label: 'Starting', tone: 'warm', Icon: Loader },
  booting: { label: 'Booting', tone: 'warm', Icon: Loader },
  ready: { label: 'Running', tone: 'good', Icon: CircleCheck },
  degraded: { label: 'Degraded', tone: 'warn', Icon: AlertTriangle },
  stopping: { label: 'Stopping', tone: 'warm', Icon: Power },
  failed: { label: 'Failed', tone: 'bad', Icon: AlertTriangle },
};

export function isActive(phase: Phase): boolean {
  return phase !== 'idle' && phase !== 'failed';
}

export function Pill({ phase }: { phase: Phase }) {
  const { label, tone, Icon } = PHASES[phase];
  const busy = phase === 'starting' || phase === 'booting' || phase === 'stopping';
  return (
    <span className={`pill ${tone}`} role="status">
      <Icon size={14} className={busy ? 'spin' : undefined} aria-hidden />
      {label}
    </span>
  );
}

/** A horizontal meter: `used` of `of`, with the unit already formatted by the caller. */
export function Meter({ used, of, label, detail }: { used: number; of: number; label: string; detail: string }) {
  const fraction = share(used, of);
  const tone = fraction > 0.9 ? 'bad' : fraction > 0.7 ? 'warn' : 'good';
  return (
    <div className="meter">
      <div className="meter-head">
        <span>{label}</span>
        <span className="faint">{detail}</span>
      </div>
      <div
        className="meter-track"
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(fraction * 100)}
      >
        <div className={`meter-fill ${tone}`} style={{ width: `${Math.max(fraction * 100, used > 0 ? 3 : 0)}%` }} />
      </div>
    </div>
  );
}

/** A ring gauge for one number that matters. */
export function Ring({ fraction, value, caption }: { fraction: number; value: string; caption: string }) {
  const radius = 42;
  const circumference = 2 * Math.PI * radius;
  const clamped = Math.min(1, Math.max(0, fraction));
  return (
    <div className="ring">
      <svg viewBox="0 0 100 100" width="112" height="112" aria-hidden>
        <circle cx="50" cy="50" r={radius} className="ring-track" />
        <circle
          cx="50"
          cy="50"
          r={radius}
          className="ring-fill"
          strokeDasharray={`${circumference * clamped} ${circumference}`}
          transform="rotate(-90 50 50)"
        />
      </svg>
      <div className="ring-text">
        <strong>{value}</strong>
        <span>{caption}</span>
      </div>
    </div>
  );
}

/** Where a boot has got to, as a row of stages. */
export function Stages({ machine, vertical = false }: { machine: Machine; vertical?: boolean }) {
  const { phase, stage } = machine.run;
  const reached = stage ? STAGES.findIndex((entry) => entry.stage === stage) : -1;
  const current = phase === 'starting' ? 0 : reached;
  const failed = phase === 'failed' || stage === 'failed';
  return (
    <ol className={`stages ${vertical ? 'vertical' : ''}`} aria-label="Boot progress">
      {STAGES.map((entry, index) => {
        const done = isActive(phase) && (index < current || (index === current && stage === 'ready'));
        const active = isActive(phase) && index === current && stage !== 'ready' && phase !== 'idle';
        const state = failed && index === Math.max(current, 0) ? 'failed' : done ? 'done' : active ? 'active' : 'todo';
        return (
          <li key={entry.stage} className={`stage ${state}`} aria-current={active ? 'step' : undefined}>
            <span className="stage-dot">
              {state === 'done' ? <CircleCheck size={18} /> : state === 'active' ? <Loader size={18} className="spin" /> : state === 'failed' ? <AlertTriangle size={18} /> : <CircleDot size={18} />}
            </span>
            <span className="stage-label">{entry.label}</span>
            <span className="stage-hint">{entry.hint}</span>
          </li>
        );
      })}
    </ol>
  );
}

export function Empty({ icon: Icon, title, children }: { icon: LucideIcon; title: string; children?: React.ReactNode }) {
  return (
    <div className="empty">
      <span className="empty-plate"><Icon size={26} /></span>
      <h3>{title}</h3>
      {children && <p className="muted">{children}</p>}
    </div>
  );
}

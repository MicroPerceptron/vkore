// The window's whole view of the engine: five commands and one event. Strings
// arrive already escaped and bounded by the Rust side, and are only ever
// rendered as text.

export type Phase = 'idle' | 'starting' | 'booting' | 'ready' | 'degraded' | 'stopping' | 'failed';
export type Stage = 'firmware' | 'kernel' | 'services' | 'shell' | 'ready' | 'degraded' | 'failed';

export interface RunState {
  phase: Phase;
  stage: Stage | null;
  message: string;
  uptimeMs: number | null;
}

export interface LogLine {
  atMs: number;
  text: string;
}

export interface Machine {
  name: string;
  iso: string;
  isoSha256: string | null;
  diskMib: number;
  cpus: number;
  memoryMib: number;
  framebuffer: [number, number];
  accel: 'auto' | 'kvm' | 'hvf' | 'tcg';
  workspace: { name: string; writeback: 'off' | 'review' | 'automatic' } | null;
  run: RunState;
}

/** A terminal colour: the default, one of 256 indexed, or `#rrggbb`. */
export type Paint = null | number | string;

export interface Span {
  text: string;
  fg: Paint;
  bg: Paint;
  bold: boolean;
  dim: boolean;
  inverse: boolean;
  underline: boolean;
  cursor: boolean;
}

/** A machine's console as the engine's terminal model left it. */
export interface ConsoleScreen {
  rows: number;
  cols: number;
  connected: boolean;
  error: string;
  applicationCursor: boolean;
  spans: Span[];
}

export interface MachineDetail extends Machine {
  lines: LogLine[];
  console: ConsoleScreen | null;
}

export interface Host {
  cpuName: string;
  physicalCores: number;
  logicalCpus: number;
  memoryMib: number;
  availableMemoryMib: number;
  storageAvailableMib: number;
  accel: 'kvm' | 'hvf' | 'tcg';
  qemu: string | null;
  running: number;
  maxRunning: number;
}

export interface Update extends RunState {
  name: string;
  line: LogLine | null;
}

export interface ConsoleUpdate extends ConsoleScreen {
  name: string;
}

export interface Api {
  host(): Promise<Host>;
  machines(): Promise<Machine[]>;
  machine(name: string): Promise<MachineDetail>;
  start(name: string): Promise<void>;
  stop(name: string): Promise<void>;
  consoleInput(name: string, text: string): Promise<void>;
  consolePaste(name: string, text: string): Promise<void>;
  consoleResize(name: string, rows: number, cols: number): Promise<void>;
  /** Each resolves to the function that stops listening. */
  onUpdate(listener: (update: Update) => void): Promise<() => void>;
  onConsole(listener: (update: ConsoleUpdate) => void): Promise<() => void>;
}

export const UPDATE_EVENT = 'machine-update';
export const CONSOLE_EVENT = 'console-screen';

async function tauri(): Promise<Api> {
  const { invoke } = await import('@tauri-apps/api/core');
  const { listen } = await import('@tauri-apps/api/event');
  return {
    host: () => invoke<Host>('host_overview'),
    machines: () => invoke<Machine[]>('machines_list'),
    machine: (name) => invoke<MachineDetail>('machine_get', { name }),
    start: (name) => invoke<void>('machine_start', { name }),
    stop: (name) => invoke<void>('machine_stop', { name }),
    consoleInput: (name, text) => invoke<void>('console_input', { name, text }),
    consolePaste: (name, text) => invoke<void>('console_paste', { name, text }),
    consoleResize: (name, rows, cols) => invoke<void>('console_resize', { name, rows, cols }),
    onUpdate: (listener) => listen<Update>(UPDATE_EVENT, (event) => listener(event.payload)),
    onConsole: (listener) => listen<ConsoleUpdate>(CONSOLE_EVENT, (event) => listener(event.payload)),
  };
}

let loaded: Promise<Api> | undefined;

/** The engine inside the app; in `vite dev` outside the app, a scripted stand-in. */
export function api(): Promise<Api> {
  loaded ??= '__TAURI_INTERNALS__' in window
    ? tauri()
    : import.meta.env.DEV
      ? import('./mock').then((mock) => mock.mockApi())
      : Promise.reject(new Error('vKore must be run inside its app.'));
  return loaded;
}

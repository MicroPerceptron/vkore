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

export interface MachineDetail extends Machine {
  lines: LogLine[];
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

export interface Api {
  host(): Promise<Host>;
  machines(): Promise<Machine[]>;
  machine(name: string): Promise<MachineDetail>;
  start(name: string): Promise<void>;
  stop(name: string): Promise<void>;
  /** Resolves to the function that stops listening. */
  onUpdate(listener: (update: Update) => void): Promise<() => void>;
}

export const UPDATE_EVENT = 'machine-update';

async function tauri(): Promise<Api> {
  const { invoke } = await import('@tauri-apps/api/core');
  const { listen } = await import('@tauri-apps/api/event');
  return {
    host: () => invoke<Host>('host_overview'),
    machines: () => invoke<Machine[]>('machines_list'),
    machine: (name) => invoke<MachineDetail>('machine_get', { name }),
    start: (name) => invoke<void>('machine_start', { name }),
    stop: (name) => invoke<void>('machine_stop', { name }),
    onUpdate: (listener) => listen<Update>(UPDATE_EVENT, (event) => listener(event.payload)),
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

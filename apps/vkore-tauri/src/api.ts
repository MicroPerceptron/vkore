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
  /** The guest's cursor as a character offset into the spans' text, when it is shown at the end of the output. */
  cursor: number | null;
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

/** A path on a volume: the root, or below a terminal's home. */
export interface Where {
  place: 'root' | 'home';
  face: number;
  names: string[];
}

export interface VolumeView {
  machine: string;
  generation: number;
  formatVersion: number;
  usedFallbackRoot: boolean;
  faces: number[];
}

export interface VolumeEntry {
  /** As the volume holds it: sent back to navigate, never drawn. */
  name: string;
  /** Made safe to draw. */
  label: string;
  directory: boolean;
  len: number;
}

export type Listing =
  | { kind: 'directory'; entries: VolumeEntry[]; truncated: boolean }
  | { kind: 'file'; len: number }
  | { kind: 'emptyHome' };

export interface Capture {
  software: string | null;
  created: string | null;
  kernelBuild: string | null;
  uptimeMs: string | null;
  mode: string | null;
  app: string | null;
}

export type Preview =
  | { kind: 'image'; width: number; height: number; len: number; dataUrl: string | null; capture: Capture | null }
  | { kind: 'text'; text: string; lines: number; truncated: boolean; len: number }
  | { kind: 'binary'; len: number; headHex: string };

export interface HardwareDraft {
  cpus: number;
  memoryMib: number;
  pinCpus: boolean;
  hostCpus: number[];
  cpuPowerManagement: boolean;
  pinMemory: boolean;
  pages: 'system' | '2m' | '1g';
  network: 'isolated' | 'e1000e';
  guestNuma: 'single' | 'split';
}

export interface HostHardware {
  logicalCpus: number;
  physicalCores: number;
  cores: { id: number; kind: string; threads: number[] }[];
  memoryMib: number;
  availableMemoryMib: number;
  cpuPinning: boolean;
  memoryPinning: boolean;
  hugetlb: boolean;
  pools: { page: 'system' | '2m' | '1g'; freeMib: number }[];
  nativeWaitsError: string | null;
}

export interface HardwareView {
  machine: string;
  draft: HardwareDraft;
  minCpus: number;
  minMemoryMib: number;
  maxCpus: number;
  diskMib: number;
  reserveStorage: boolean;
  devices: number;
  host: HostHardware;
}

export interface HardwareCheck {
  ok: boolean;
  message: string;
  needsAuthorization: boolean;
}

export interface ConsoleUpdate extends ConsoleScreen {
  name: string;
}

export interface AgentAsk {
  id: string;
  title: string;
  /** Exactly what would be typed or touched. */
  detail: string;
  /** What "allow for this session" would cover, when it may be offered. */
  remember: 'exact' | 'kind' | null;
}

export type AgentEntry =
  | { kind: 'prompt'; id: number; text: string }
  | { kind: 'reply'; id: number; text: string }
  | { kind: 'action'; id: number; title: string; detail: string; state: 'pending' | 'done' | 'failed' | 'denied' | 'stopped'; output: string }
  | { kind: 'note'; id: number; text: string; bad: boolean };

/** One machine's agent, as the app keeps it. */
export interface AgentView {
  name: string;
  configured: boolean;
  /** Why no model is configured, when none is. */
  problem: string;
  model: string;
  host: string;
  /** The model's endpoint is on this computer. */
  local: boolean;
  enabled: boolean;
  busy: boolean;
  entries: AgentEntry[];
  ask: AgentAsk | null;
}

export type Choice = 'once' | 'session' | 'deny';

export interface Api {
  host(): Promise<Host>;
  machines(): Promise<Machine[]>;
  machine(name: string): Promise<MachineDetail>;
  start(name: string): Promise<void>;
  stop(name: string): Promise<void>;
  consoleInput(name: string, text: string): Promise<void>;
  consolePaste(name: string, text: string): Promise<void>;
  consoleResize(name: string, rows: number, cols: number): Promise<void>;
  hardwareGet(name: string): Promise<HardwareView>;
  hardwareCheck(name: string, draft: HardwareDraft): Promise<HardwareCheck>;
  /** May wait on the desktop's authentication agent. */
  hardwareSave(name: string, draft: HardwareDraft): Promise<void>;
  volumeOpen(name: string): Promise<VolumeView>;
  volumeList(name: string, at: Where): Promise<Listing>;
  volumePreview(name: string, at: Where): Promise<Preview>;
  volumeClose(): Promise<void>;
  agentView(name: string): Promise<AgentView>;
  agentEnable(name: string): Promise<void>;
  agentDisable(name: string): Promise<void>;
  agentSend(name: string, text: string): Promise<void>;
  agentDecide(name: string, ask: string, choice: Choice): Promise<void>;
  agentStop(name: string): Promise<void>;
  /** Each resolves to the function that stops listening. */
  onUpdate(listener: (update: Update) => void): Promise<() => void>;
  onConsole(listener: (update: ConsoleUpdate) => void): Promise<() => void>;
  onAgent(listener: (view: AgentView) => void): Promise<() => void>;
}

export const UPDATE_EVENT = 'machine-update';
export const CONSOLE_EVENT = 'console-screen';
export const AGENT_EVENT = 'agent-update';

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
    hardwareGet: (name) => invoke<HardwareView>('hardware_get', { name }),
    hardwareCheck: (name, draft) => invoke<HardwareCheck>('hardware_check', { name, draft }),
    hardwareSave: (name, draft) => invoke<void>('hardware_save', { name, draft }),
    volumeOpen: (name) => invoke<VolumeView>('volume_open', { name }),
    volumeList: (name, at) => invoke<Listing>('volume_list', { name, at }),
    volumePreview: (name, at) => invoke<Preview>('volume_preview', { name, at }),
    volumeClose: () => invoke<void>('volume_close'),
    agentView: (name) => invoke<AgentView>('agent_view', { name }),
    agentEnable: (name) => invoke<void>('agent_enable', { name }),
    agentDisable: (name) => invoke<void>('agent_disable', { name }),
    agentSend: (name, text) => invoke<void>('agent_send', { name, text }),
    agentDecide: (name, ask, choice) => invoke<void>('agent_decide', { name, ask, choice }),
    agentStop: (name) => invoke<void>('agent_stop', { name }),
    onAgent: (listener) => listen<AgentView>(AGENT_EVENT, (event) => listener(event.payload)),
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
      ? import('./mock').then((mock) => mock.mockApiWithVolumes())
      : Promise.reject(new Error('vKore must be run inside its app.'));
  return loaded;
}

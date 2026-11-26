// A scripted engine for `vite dev` outside the app, so the interface can be
// looked at and driven in an ordinary browser. It is compiled out of the
// production build (`import.meta.env.DEV`), and is not the engine's behaviour:
// real boots come from `vkored::Session`.
import type { Api, ConsoleScreen, ConsoleUpdate, Host, LogLine, Machine, Phase, Span, Stage, Update, Where } from './api';

const host: Host = {
  cpuName: 'AMD Ryzen 9 7950X 16-Core Processor',
  physicalCores: 16,
  logicalCpus: 32,
  memoryMib: 65536,
  availableMemoryMib: 48000,
  storageAvailableMib: 812_000,
  accel: 'kvm',
  qemu: 'qemu-system-x86_64',
  running: 0,
  maxRunning: 8,
};

const idle = { phase: 'idle' as Phase, stage: null, message: '', uptimeMs: null };

const machines: Machine[] = [
  {
    name: 'kore-dev',
    iso: 'kore-0.11.0-console.iso',
    isoSha256: '9f2c41d0a7be',
    diskMib: 20480,
    cpus: 4,
    memoryMib: 4096,
    framebuffer: [1280, 800],
    accel: 'auto',
    workspace: { name: 'vkore', writeback: 'review' },
    run: { ...idle },
  },
  {
    name: 'inference',
    iso: 'kore-0.11.0.iso',
    isoSha256: '31ab9e7740cc',
    diskMib: 65536,
    cpus: 8,
    memoryMib: 16384,
    framebuffer: [1920, 1080],
    accel: 'kvm',
    workspace: null,
    run: { ...idle },
  },
  {
    name: 'scratch',
    iso: 'kore-0.10.2-console.iso',
    isoSha256: 'c04d77e1b3a9',
    diskMib: 8192,
    cpus: 2,
    memoryMib: 2048,
    framebuffer: [1024, 768],
    accel: 'tcg',
    workspace: { name: 'notes', writeback: 'off' },
    run: { ...idle },
  },
  {
    name: 'nightly',
    iso: 'kore-nightly.iso',
    isoSha256: null,
    diskMib: 32768,
    cpus: 6,
    memoryMib: 8192,
    framebuffer: [1280, 800],
    accel: 'auto',
    workspace: { name: 'vkore', writeback: 'automatic' },
    run: { ...idle, phase: 'failed', message: 'the ISO changed since this machine last booted it' },
  },
];

const BOOT: [Stage, number, string][] = [
  ['firmware', 700, 'firmware: UEFI handed over to the loader'],
  ['kernel', 1500, 'kernel: console is up'],
  ['services', 2600, 'services: every row founded and ready'],
  ['shell', 3400, 'shell: the console answers'],
  ['ready', 4300, 'applications seeded; terminal homes exist'],
];

const logs = new Map<string, LogLine[]>();
const timers = new Map<string, number[]>();
const listeners = new Set<(update: Update) => void>();
const consoleListeners = new Set<(update: ConsoleUpdate) => void>();
const shells = new Map<string, { history: string; line: string }>();

const PROMPT = 'kore> ';

function span(text: string, over: Partial<Span> = {}): Span {
  return { text, fg: null, bg: null, bold: false, dim: false, inverse: false, underline: false, cursor: false, ...over };
}

function screenOf(name: string, connected = true): ConsoleScreen {
  const shell = shells.get(name) ?? { history: '', line: '' };
  return {
    rows: 24,
    cols: 100,
    connected,
    error: '',
    applicationCursor: false,
    spans: [
      span(shell.history),
      span(PROMPT, { fg: 6, bold: true }),
      span(shell.line),
      span(' ', { cursor: true }),
    ],
  };
}

function publishConsole(name: string, connected = true) {
  const screen = screenOf(name, connected);
  for (const listener of consoleListeners) listener({ name, ...screen });
}

function runLine(name: string, line: string) {
  const shell = shells.get(name);
  if (!shell) return;
  const [command = '', ...rest] = line.trim().split(/\s+/);
  const out: Record<string, string> = {
    help: 'commands: help, ls, echo, uname, clear\n',
    ls: 'applications/  pictures/  documents/  vkore/\n',
    uname: 'Kore 0.11.0 x86_64\n',
    echo: `${rest.join(' ')}\n`,
  };
  shell.history += `${PROMPT}${line}\n`;
  if (command === 'clear') shell.history = '';
  else if (command) shell.history += out[command] ?? `${command}: not found\n`;
  shell.line = '';
}

function find(name: string): Machine {
  const found = machines.find((machine) => machine.name === name);
  if (!found) throw new Error(`no machine named ${name}`);
  return found;
}

function emit(machine: Machine, line: string | null, began: number) {
  let log: LogLine | null = null;
  if (line !== null) {
    log = { atMs: Date.now() - began, text: line };
    const kept = logs.get(machine.name) ?? [];
    kept.push(log);
    logs.set(machine.name, kept.slice(-200));
  }
  for (const listener of listeners) listener({ name: machine.name, ...machine.run, line: log });
}

const wait = (ms: number) => new Promise((done) => setTimeout(done, ms));

type Base = Omit<Api, 'volumeOpen' | 'volumeList' | 'volumePreview' | 'volumeClose'>;

function mockApi(): Base {
  return {
    async host() {
      await wait(60);
      return { ...host, running: machines.filter((m) => isActive(m.run.phase)).length };
    },
    async machines() {
      await wait(60);
      return structuredClone(machines);
    },
    async machine(name) {
      await wait(30);
      return {
        ...structuredClone(find(name)),
        lines: logs.get(name) ?? [],
        console: shells.has(name) ? screenOf(name, isActive(find(name).run.phase)) : null,
      };
    },
    async start(name) {
      const machine = find(name);
      if (isActive(machine.run.phase)) throw new Error('that machine is already running');
      const began = Date.now();
      logs.set(name, []);
      machine.run = { phase: 'starting', stage: null, message: '', uptimeMs: null };
      emit(machine, 'Checking hardware settings and permissions…', began);
      const pending: number[] = [];
      const at = (ms: number, work: () => void) => pending.push(window.setTimeout(work, ms));
      at(350, () => emit(machine, `preflight: image ${machine.isoSha256 ?? 'unknown'} on Kvm`, began));
      at(550, () => {
        machine.run = { ...machine.run, phase: 'booting' };
        emit(machine, 'QEMU started (pid 48213)', began);
      });
      for (const [stage, ms, text] of BOOT) {
        at(ms + 600, () => {
          machine.run = {
            phase: stage === 'ready' ? 'ready' : 'booting',
            stage,
            message: '',
            uptimeMs: ms,
          };
          emit(machine, text, began);
          if (stage === 'shell' && machine.iso.includes('console')) {
            shells.set(name, { history: 'Kore 0.11.0 console\ntype help for commands\n', line: '' });
            publishConsole(name);
          }
        });
      }
      timers.set(name, pending);
    },
    async stop(name) {
      const machine = find(name);
      if (!isActive(machine.run.phase)) throw new Error('that machine is not running');
      for (const id of timers.get(name) ?? []) clearTimeout(id);
      const began = Date.now();
      machine.run = { ...machine.run, phase: 'stopping' };
      emit(machine, 'stopping', began);
      window.setTimeout(() => {
        if (shells.has(name)) publishConsole(name, false);
        machine.run = { phase: 'idle', stage: null, message: 'stopped', uptimeMs: null };
        emit(machine, 'stopped', began);
      }, 1200);
    },
    async consoleInput(name, text) {
      const shell = shells.get(name);
      if (!shell || !isActive(find(name).run.phase)) throw new Error('that machine has no console open');
      for (const char of text) {
        if (char === '\r') runLine(name, shell.line);
        else if (char === '\x7f') shell.line = shell.line.slice(0, -1);
        else if (char >= ' ' && !char.startsWith('\x1b')) shell.line += char;
      }
      publishConsole(name);
    },
    async consolePaste(name, text) {
      const shell = shells.get(name);
      if (!shell) throw new Error('that machine has no console open');
      shell.line += text.replace(/[\r\n]+/g, ' ');
      publishConsole(name);
    },
    async consoleResize() {},
    async onConsole(listener) {
      consoleListeners.add(listener);
      return () => consoleListeners.delete(listener);
    },
    async onUpdate(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

function isActive(phase: Phase) {
  return phase !== 'idle' && phase !== 'failed';
}

// ── Volumes: a small scripted DagFS tree per machine, for the interface alone ──
const SHOT =
  'data:image/svg+xml;utf8,' +
  encodeURIComponent(
    '<svg xmlns="http://www.w3.org/2000/svg" width="320" height="200"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#1c2a3a"/><stop offset="1" stop-color="#2a9d8f"/></linearGradient></defs><rect width="320" height="200" fill="url(#g)"/><rect x="24" y="24" width="272" height="30" rx="6" fill="#ef6151"/><rect x="24" y="70" width="180" height="14" rx="4" fill="#f0e8dc"/><rect x="24" y="94" width="220" height="14" rx="4" fill="#f0e8dc" opacity=".6"/></svg>',
  );

interface Dir { dir: Record<string, Node> }
interface Leaf { len: number; kind: 'text' | 'image' | 'binary'; body?: string }
type Node = Dir | Leaf;
const tree: Record<string, Node> = {
  proj: {
    dir: {
      'notes.txt': { len: 148, kind: 'text', body: 'vKore project notes\n\n- boot the console image\n- run the checks\n- export the screenshot\n' },
      'Cargo.toml': { len: 212, kind: 'text', body: '[package]\nname = "demo"\nversion = "0.1.0"\n' },
      'kore.bin': { len: 4096, kind: 'binary', body: '4b4f5245010000000000000000100000deadbeef' },
      src: { dir: { 'main.rs': { len: 64, kind: 'text', body: 'fn main() {\n    println!("hello from kore");\n}\n' } } },
    },
  },
  pictures: {
    dir: { screenshots: { dir: { 'capture-001.png': { len: 48213, kind: 'image' }, 'capture-002.png': { len: 51877, kind: 'image' } } } },
  },
};

function walk(names: string[], at: Where): Node | null {
  if (at.place === 'root') return names.length === 0 ? { dir: { work: { dir: { 'face-0': { dir: tree } } } } } : null;
  let node: Node = { dir: tree };
  for (const name of names) {
    if (!('dir' in node)) return null;
    const next: Node | undefined = node.dir[name];
    if (!next) return null;
    node = next;
  }
  return node;
}

const baseApi = mockApi;
export function mockApiWithVolumes(): Api {
  const api = baseApi();
  let open: string | null = null;
  const require = (name: string) => {
    if (isActive(find(name).run.phase)) throw new Error('a running machine owns its disk; stop it to look inside');
    if (name === 'nightly') throw new Error('this disk is blank: the machine formats it at its first boot');
    open = name;
  };
  return {
    ...api,
    async volumeOpen(name) {
      require(name);
      return { machine: name, generation: 42, formatVersion: 3, usedFallbackRoot: name === 'scratch', faces: [0, 1, 2, 3] };
    },
    async volumeList(name, at) {
      require(name);
      const node = walk(at.names, at.face === 0 || at.place === 'root' ? at : { ...at, names: [] });
      if (at.place === 'home' && at.face !== 0) return { kind: 'emptyHome' };
      if (!node) throw new Error('not found');
      if (!('dir' in node)) return { kind: 'file', len: node.len };
      const entries = Object.entries(node.dir)
        .map(([entry, child]) => ({ name: entry, label: entry, directory: 'dir' in child, len: 'len' in child ? child.len : 0 }))
        .sort((a, b) => Number(b.directory) - Number(a.directory) || a.name.localeCompare(b.name));
      return { kind: 'directory', entries, truncated: false };
    },
    async volumePreview(name, at) {
      require(name);
      const node = walk(at.names, at);
      if (!node || 'dir' in node) throw new Error('that is not a file');
      if (node.kind === 'image') {
        return { kind: 'image', width: 320, height: 200, len: node.len, dataUrl: SHOT, capture: { software: 'kore 0.11.0', created: '2026-11-25T21:04:11Z', kernelBuild: 'a1b2c3d', uptimeMs: '48213', mode: 'screen', app: null } };
      }
      if (node.kind === 'binary') return { kind: 'binary', len: node.len, headHex: node.body ?? '' };
      return { kind: 'text', text: node.body ?? '', lines: (node.body ?? '').split('\n').length - 1, truncated: false, len: node.len };
    },
    async volumeClose() {
      open = null;
      void open;
    },
  };
}

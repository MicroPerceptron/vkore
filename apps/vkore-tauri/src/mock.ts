// A scripted engine for `vite dev` outside the app, so the interface can be
// looked at and driven in an ordinary browser. It is compiled out of the
// production build (`import.meta.env.DEV`), and is not the engine's behaviour:
// real boots come from `vkored::Session`.
import type { Api, ConsoleScreen, ConsoleUpdate, Host, LogLine, Machine, Phase, Span, Stage, Update } from './api';

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

export function mockApi(): Api {
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

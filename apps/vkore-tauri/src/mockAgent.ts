// A scripted agent for working on the interface in a browser; never shipped.
import type { AgentEntry, AgentView, Api, Choice } from './api';

type AgentApi = Pick<Api, 'agentView' | 'agentEnable' | 'agentDisable' | 'agentSend' | 'agentDecide' | 'agentStop' | 'onAgent'>;

interface Live {
  entries: AgentEntry[];
  busy: boolean;
  ask: AgentView['ask'];
  timers: number[];
}

export function mockAgent(type: (name: string, input: string) => Promise<void>): AgentApi {
  const agents = new Map<string, Live>();
  const listeners = new Set<(view: AgentView) => void>();
  let next = 0;

  const view = (name: string): AgentView => {
    const live = agents.get(name);
    return {
      name, configured: true, problem: '', model: 'scripted-model', host: 'api.example.com', local: false,
      enabled: !!live, busy: live?.busy ?? false, entries: structuredClone(live?.entries ?? []), ask: live?.ask ?? null,
    };
  };
  const publish = (name: string) => {
    for (const listener of listeners) listener(view(name));
  };
  const later = (name: string, ms: number, work: (live: Live) => void) => {
    const live = agents.get(name);
    if (!live) return;
    live.timers.push(window.setTimeout(() => {
      work(live);
      publish(name);
    }, ms));
  };
  const get = (name: string) => {
    const live = agents.get(name);
    if (!live) throw new Error('the agent is off for this machine');
    return live;
  };
  const stream = (name: string, text: string, after: (live: Live) => void) => {
    const id = ++next;
    const words = text.split(' ');
    words.forEach((_, index) => later(name, 60 * (index + 1), (live) => {
      const entry: AgentEntry = { kind: 'reply', id, text: words.slice(0, index + 1).join(' ') };
      const at = live.entries.findIndex((e) => e.id === id);
      if (at < 0) live.entries.push(entry);
      else live.entries[at] = entry;
      if (index === words.length - 1) after(live);
    }));
  };

  return {
    async agentView(name) {
      return view(name);
    },
    async agentEnable(name) {
      if (!agents.has(name)) agents.set(name, { entries: [], busy: false, ask: null, timers: [] });
      publish(name);
    },
    async agentDisable(name) {
      agents.get(name)?.timers.forEach(clearTimeout);
      agents.delete(name);
      publish(name);
    },
    async agentSend(name, text) {
      const live = get(name);
      if (live.busy) throw new Error('the agent is still working');
      live.busy = true;
      live.entries.push({ kind: 'prompt', id: ++next, text });
      publish(name);
      stream(name, 'I will ask the shell what it offers.', (state) => {
        const id = ++next;
        state.entries.push({ kind: 'action', id, title: 'Type into the console', detail: 'help⏎', state: 'pending', output: '' });
        state.ask = { id: `ask-${id}`, title: 'Type into the console', detail: 'help⏎', remember: /again/i.test(text) ? 'exact' : null };
      });
    },
    async agentDecide(name, ask, choice: Choice) {
      const live = get(name);
      if (live.ask?.id !== ask) throw new Error('that request is no longer waiting');
      live.ask = null;
      const action = [...live.entries].reverse().find((e) => e.kind === 'action');
      if (action?.kind !== 'action') return;
      let failed = false;
      if (choice !== 'deny') {
        try {
          await type(name, 'help\r');
        } catch (error) {
          failed = true;
          action.output = `It failed: ${error instanceof Error ? error.message : error}`;
        }
      }
      action.state = choice === 'deny' ? 'denied' : failed ? 'failed' : 'done';
      action.output ||= choice === 'deny' ? 'Not run: the person refused it.' : 'The console is open. Its newest output:\nkore> help\ncommands: help, ls, echo, uname, clear';
      publish(name);
      const reply = choice === 'deny'
        ? 'Understood, I will not type it. I wanted to list the shell’s commands.'
        : failed
          ? 'The machine has no open console yet. Start it, then ask me again.'
          : 'The shell offers help, ls, echo, uname and clear.';
      stream(name, reply, (state) => {
        state.busy = false;
      });
    },
    async agentStop(name) {
      const live = get(name);
      if (!live.busy) return;
      live.timers.forEach(clearTimeout);
      live.busy = false;
      live.ask = null;
      for (const entry of live.entries) if (entry.kind === 'action' && entry.state === 'pending') entry.state = 'stopped';
      live.entries.push({ kind: 'note', id: ++next, text: 'Stopped.', bad: false });
      publish(name);
    },
    async onAgent(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

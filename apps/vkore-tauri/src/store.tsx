import { createContext, useCallback, useContext, useEffect, useMemo, useReducer, useRef, type ReactNode } from 'react';
import { api, type Host, type LogLine, type Machine, type Update } from './api';

const MAX_LINES = 200;

interface State {
  host: Host | null;
  machines: Machine[] | null;
  logs: Record<string, LogLine[]>;
  error: string | null;
}

type Action =
  | { kind: 'loaded'; host: Host; machines: Machine[] }
  | { kind: 'failed'; error: string }
  | { kind: 'update'; update: Update }
  | { kind: 'log'; name: string; lines: LogLine[] };

export function reduce(state: State, action: Action): State {
  switch (action.kind) {
    case 'loaded':
      return { ...state, host: action.host, machines: action.machines, error: null };
    case 'failed':
      return { ...state, error: action.error };
    case 'log':
      return { ...state, logs: { ...state.logs, [action.name]: action.lines.slice(-MAX_LINES) } };
    case 'update': {
      const { update } = action;
      const machines = state.machines?.map((machine) =>
        machine.name === update.name
          ? {
              ...machine,
              run: {
                phase: update.phase,
                stage: update.stage,
                message: update.message,
                uptimeMs: update.uptimeMs,
              },
            }
          : machine,
      ) ?? null;
      const logs = update.line
        ? {
            ...state.logs,
            [update.name]: [...(state.logs[update.name] ?? []), update.line].slice(-MAX_LINES),
          }
        : state.logs;
      const host = state.host && machines
        ? { ...state.host, running: machines.filter((m) => m.run.phase !== 'idle' && m.run.phase !== 'failed').length }
        : state.host;
      return { ...state, machines, logs, host };
    }
  }
}

interface Store extends State {
  refresh(): Promise<void>;
  start(name: string): Promise<string | null>;
  stop(name: string): Promise<string | null>;
  loadLog(name: string): Promise<void>;
}

const Context = createContext<Store | null>(null);

export function StoreProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reduce, { host: null, machines: null, logs: {}, error: null });
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const engine = await api();
      const [host, machines] = await Promise.all([engine.host(), engine.machines()]);
      if (mounted.current) dispatch({ kind: 'loaded', host, machines });
    } catch (error) {
      if (mounted.current) dispatch({ kind: 'failed', error: String(error) });
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    let stop: (() => void) | undefined;
    void refresh();
    void api()
      .then((engine) => engine.onUpdate((update) => dispatch({ kind: 'update', update })))
      .then((unlisten) => {
        if (mounted.current) stop = unlisten;
        else unlisten();
      })
      .catch((error) => dispatch({ kind: 'failed', error: String(error) }));
    return () => {
      mounted.current = false;
      stop?.();
    };
  }, [refresh]);

  const value = useMemo<Store>(
    () => ({
      ...state,
      refresh,
      start: async (name) => {
        try {
          await (await api()).start(name);
          return null;
        } catch (error) {
          return String(error);
        }
      },
      stop: async (name) => {
        try {
          await (await api()).stop(name);
          return null;
        } catch (error) {
          return String(error);
        }
      },
      loadLog: async (name) => {
        try {
          const detail = await (await api()).machine(name);
          if (mounted.current) dispatch({ kind: 'log', name, lines: detail.lines });
        } catch {
          // The list already carries the failure; an empty log is the honest view.
        }
      },
    }),
    [state, refresh],
  );
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

export function useStore(): Store {
  const store = useContext(Context);
  if (!store) throw new Error('useStore outside StoreProvider');
  return store;
}

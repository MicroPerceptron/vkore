import { Bot, Check, CircleSlash, Loader, OctagonX, Power, SendHorizontal, ShieldQuestion, Square, TriangleAlert } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { api, type AgentAsk, type AgentEntry, type AgentView, type Choice } from './api';
import { Empty } from './ui';

/** A button that answers a request cannot be pressed this soon after it appears,
 * so a click aimed at something else never approves anything. */
export const ARM_MS = 500;

const STATES = {
  pending: { label: 'waiting', tone: 'warm', Icon: Loader },
  done: { label: 'done', tone: 'good', Icon: Check },
  failed: { label: 'failed', tone: 'bad', Icon: TriangleAlert },
  denied: { label: 'refused', tone: 'warn', Icon: CircleSlash },
  stopped: { label: 'stopped', tone: 'quiet', Icon: OctagonX },
} as const;

/** What "for this session" covers, in the words on its button. */
export function rememberLabel(ask: AgentAsk): string | null {
  if (ask.remember === 'exact') return 'Allow this exact action for the session';
  if (ask.remember === 'kind') return `Allow every “${ask.title}” for the session`;
  return null;
}

function EntryRow({ entry }: { entry: AgentEntry }) {
  switch (entry.kind) {
    case 'prompt':
      return <li className="said me">{entry.text}</li>;
    case 'reply':
      return <li className="said">{entry.text}</li>;
    case 'note':
      return <li className={`agent-note ${entry.bad ? 'bad' : ''}`}>{entry.text}</li>;
    case 'action': {
      const { label, tone, Icon } = STATES[entry.state];
      return (
        <li className="act">
          <div className="act-head">
            <strong>{entry.title}</strong>
            <span className={`pill ${tone}`}><Icon size={13} /> {label}</span>
          </div>
          {entry.detail && <code className="act-detail">{entry.detail}</code>}
          {entry.output && (
            <details>
              <summary>What the model was told</summary>
              <pre className="act-output">{entry.output}</pre>
            </details>
          )}
        </li>
      );
    }
  }
}

function Request({ ask, decide }: { ask: AgentAsk; decide: (choice: Choice) => void }) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    setArmed(false);
    const timer = setTimeout(() => setArmed(true), ARM_MS);
    return () => clearTimeout(timer);
  }, [ask.id]);
  const remember = rememberLabel(ask);
  // No button is focused for the person: Enter in the message box must not approve.
  return (
    <div className="request" role="group" aria-label="The agent is asking for approval">
      <p className="request-title"><ShieldQuestion size={16} /> {ask.title}?</p>
      {ask.detail && <code className="act-detail">{ask.detail}</code>}
      <div className="request-actions">
        <button className="primary" disabled={!armed} onClick={() => decide('once')}>Allow once</button>
        <button disabled={!armed} onClick={() => decide('deny')}>Refuse</button>
        {remember && <button disabled={!armed} onClick={() => decide('session')}>{remember}</button>}
      </div>
    </div>
  );
}

function Setup({ problem }: { problem: string }) {
  return (
    <div className="agent-setup">
      <Empty icon={Bot} title="No model is configured">
        {problem}. Set these before starting vKore, then start it again.
      </Empty>
      <pre className="text-preview">{`VKORE_AGENT_BASE_URL   an OpenAI-compatible address, usually ending in /v1
VKORE_AGENT_MODEL      the model's name there
VKORE_AGENT_API_KEY    its key, if it wants one`}</pre>
    </div>
  );
}

/** The operator agent for one machine. */
export function AgentPanel({ name }: { name: string }) {
  const [view, setView] = useState<AgentView | null>(null);
  const [draft, setDraft] = useState('');
  const [error, setError] = useState('');
  const log = useRef<HTMLOListElement>(null);
  // Whether the reader is at the newest entry. Kept from their scrolling, since a
  // request card appearing below shrinks the list before it can be asked.
  const following = useRef(true);

  useEffect(() => {
    let live = true;
    let heard = false;
    let off: (() => void) | undefined;
    setView(null);
    setError('');
    void (async () => {
      const engine = await api();
      const unlisten = await engine.onAgent((next) => {
        if (next.name !== name || !live) return;
        heard = true;
        setView(next);
      });
      if (!live) return unlisten();
      off = unlisten;
      const first = await engine.agentView(name);
      // An update that arrived meanwhile is newer than this answer.
      if (live && !heard) setView(first);
    })().catch((failure) => live && setError(String(failure)));
    return () => {
      live = false;
      off?.();
    };
  }, [name]);

  // Follow the conversation, unless the reader has scrolled up to read.
  useEffect(() => {
    const element = log.current;
    if (element && following.current) element.scrollTop = element.scrollHeight;
  }, [view?.entries, view?.ask]);

  const act = useCallback(async (work: (engine: Awaited<ReturnType<typeof api>>) => Promise<void>) => {
    try {
      await work(await api());
      setError('');
      return true;
    } catch (failure) {
      setError(String(failure));
      return false;
    }
  }, []);

  const send = async () => {
    const text = draft.trim();
    if (!text || view?.busy) return;
    following.current = true;
    if (await act((engine) => engine.agentSend(name, text))) setDraft('');
  };

  if (!view) return <p className="muted agent-pad">{error || 'Reading the agent…'}</p>;
  if (!view.configured) return <Setup problem={view.problem} />;
  const where = view.local ? 'a model on this computer' : view.host;

  if (!view.enabled) {
    return (
      <div className="agent-intro">
        <span className="plate big"><Bot size={24} /></span>
        <h3>Let {view.model} operate {name}</h3>
        <p className="muted">
          While it is on, the agent can read this machine&apos;s boot events, console and files.
          {view.local ? ' What it reads goes to ' : ' What it reads is sent to '}
          <strong>{where}</strong>
          {view.local ? ' and stays on this computer.' : ', outside this computer.'}
        </p>
        <p className="muted">It asks you before it starts, stops or types anything.</p>
        <button className="primary" onClick={() => void act((engine) => engine.agentEnable(name))}>
          <Power size={15} /> Turn on for {name}
        </button>
        {error && <p className="notice bad" role="alert">{error}</p>}
      </div>
    );
  }

  return (
    <div className="agent">
      <header className="agent-head">
        <span className="faint small" title={view.local ? 'This model runs on this computer' : 'What the agent reads is sent here'}>
          {view.model} · {view.local ? 'this computer' : view.host}
        </span>
        <button
          title="Turn the agent off: its conversation and remembered approvals are forgotten"
          onClick={() => void act((engine) => engine.agentDisable(name))}
        >
          <Power size={14} /> Turn off
        </button>
      </header>

      <ol
        className="talk"
        ref={log}
        aria-label="Conversation"
        aria-live="polite"
        onScroll={({ currentTarget: list }) => {
          following.current = list.scrollHeight - list.scrollTop - list.clientHeight < 40;
        }}
      >
        {view.entries.length === 0 && (
          <li className="agent-note">Ask about {name}, or ask it to do something. For example: “Boot it and tell me what help lists.”</li>
        )}
        {view.entries.map((entry) => <EntryRow key={entry.id} entry={entry} />)}
        {view.busy && !view.ask && <li className="agent-note working"><Loader size={14} /> Working…</li>}
      </ol>

      {view.ask && <Request ask={view.ask} decide={(choice) => void act((engine) => engine.agentDecide(name, view.ask!.id, choice))} />}
      {error && <p className="inline-error agent-pad" role="alert">{error}</p>}

      <form
        className="compose"
        onSubmit={(event) => {
          event.preventDefault();
          void send();
        }}
      >
        <textarea
          aria-label="Message to the agent"
          placeholder={view.busy ? 'The agent is working' : 'Message the agent'}
          rows={2}
          value={draft}
          maxLength={8000}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault();
              void send();
            }
          }}
        />
        {view.busy ? (
          <button type="button" onClick={() => void act((engine) => engine.agentStop(name))}><Square size={13} /> Stop</button>
        ) : (
          <button type="submit" className="primary icon" aria-label="Send" title="Send" disabled={!draft.trim()}><SendHorizontal size={16} /></button>
        )}
      </form>
    </div>
  );
}

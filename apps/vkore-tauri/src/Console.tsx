import { SquareTerminal } from 'lucide-react';
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ClipboardEvent, type CSSProperties, type KeyboardEvent } from 'react';
import { api, type ConsoleScreen, type Span } from './api';
import { keyToInput, paintCss } from './keys';
import { Empty } from './ui';

function style(span: Span): CSSProperties {
  let fg = paintCss(span.fg);
  let bg = paintCss(span.bg);
  if (span.inverse || span.cursor) {
    [fg, bg] = [bg ?? 'var(--term-bg)', fg ?? 'var(--term-fg)'];
  }
  return {
    color: fg ?? undefined,
    backgroundColor: bg ?? undefined,
    fontWeight: span.bold ? 600 : undefined,
    opacity: span.dim ? 0.65 : undefined,
    textDecoration: span.underline ? 'underline' : undefined,
  };
}

/**
 * A machine's console. The engine's terminal model has already parsed the guest's
 * bytes, so this only draws styled text, and turns key presses into terminal input.
 */
export function Console({ name, screen, running, fill = false }: { name: string; screen: ConsoleScreen | undefined; running: boolean; fill?: boolean }) {
  const view = useRef<HTMLDivElement>(null);
  const meter = useRef<HTMLSpanElement>(null);
  const [problem, setProblem] = useState('');
  const [focused, setFocused] = useState(false);
  const queue = useRef({ text: '', scheduled: false, chain: Promise.resolve() });
  const grid = useRef({ rows: 0, cols: 0 });

  const connected = !!screen?.connected;

  // Send typed text in order, batching what arrives in the same tick.
  const send = useCallback(
    (text: string) => {
      const q = queue.current;
      q.text += text;
      if (q.scheduled) return;
      q.scheduled = true;
      setTimeout(() => {
        const chunk = q.text;
        q.text = '';
        q.scheduled = false;
        q.chain = q.chain.then(async () => {
          try {
            await (await api()).consoleInput(name, chunk);
            setProblem('');
          } catch (error) {
            setProblem(String(error));
          }
        });
      }, 0);
    },
    [name],
  );

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!connected) return;
    const input = keyToInput(event, screen?.applicationCursor ?? false);
    if (input === null) return;
    event.preventDefault();
    send(input);
  };

  const onPaste = (event: ClipboardEvent<HTMLDivElement>) => {
    event.preventDefault();
    const text = event.clipboardData.getData('text');
    if (!connected || !text) return;
    api()
      .then((engine) => engine.consolePaste(name, text))
      .then(() => setProblem(''))
      .catch((error) => setProblem(String(error)));
  };

  // Follow the newest output unless the reader has scrolled up.
  const spans = screen?.spans;
  useLayoutEffect(() => {
    const element = view.current;
    if (element && element.scrollHeight - element.scrollTop - element.clientHeight < 60) {
      element.scrollTop = element.scrollHeight;
    }
  }, [spans]);

  // Tell the engine how many rows and columns fit, once the size settles.
  useEffect(() => {
    const element = view.current;
    const probe = meter.current;
    if (!element || !probe || !connected) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const fit = () => {
      const box = probe.getBoundingClientRect();
      if (box.width === 0 || box.height === 0) return;
      const cols = Math.max(20, Math.floor((element.clientWidth - 28) / (box.width / 10)));
      const rows = Math.max(5, Math.floor((element.clientHeight - 20) / box.height));
      if (cols === grid.current.cols && rows === grid.current.rows) return;
      grid.current = { rows, cols };
      void api().then((engine) => engine.consoleResize(name, rows, cols)).catch(() => undefined);
    };
    const observer = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(fit, 150);
    });
    observer.observe(element);
    fit();
    return () => {
      clearTimeout(timer);
      observer.disconnect();
    };
  }, [name, connected]);

  if (!screen) {
    return (
      <Empty icon={SquareTerminal} title={running ? 'Waiting for the console…' : 'No console yet'}>
        {running
          ? 'A console image opens its shell on COM2 once it has booted. A graphical image has none, and this window does not show a display yet.'
          : 'Start the machine to open its shell here.'}
      </Empty>
    );
  }

  return (
    <div className={fill ? 'console-fill' : undefined}>
      <div
        ref={view}
        className={`term ${focused ? 'focus' : ''} ${connected ? '' : 'closed'}`}
        tabIndex={0}
        role="textbox"
        aria-multiline="true"
        aria-readonly={!connected}
        aria-label={`Console of ${name}`}
        onKeyDown={onKeyDown}
        onPaste={onPaste}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
      >
        <span ref={meter} className="term-meter" aria-hidden>MMMMMMMMMM</span>
        <pre>
          {screen.spans.map((span, index) => (
            <span key={index} style={style(span)} className={span.cursor ? 'cursor' : undefined}>
              {span.text}
            </span>
          ))}
        </pre>
      </div>
      <p className="term-note faint small" aria-live="polite">
        {!connected
          ? screen.error || 'The console is closed.'
          : problem || screen.error || (focused ? 'Typing goes to the machine. Shift+Tab leaves the console.' : 'Click the console to type.')}
      </p>
    </div>
  );
}

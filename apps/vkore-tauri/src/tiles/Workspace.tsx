import { Check, LayoutPanelLeft, Maximize2, Minimize2, Plus, RotateCcw, X } from 'lucide-react';
import { DockviewReact, themeLightSpaced, type DockviewApi, type DockviewReadyEvent, type DockviewTheme, type IDockviewHeaderActionsProps, type IDockviewPanelProps } from 'dockview-react';
import { useEffect, useMemo, useRef, useState, type FC } from 'react';
import type { Machine } from '../api';
import { StartStop } from '../pages/Machines';
import { Pill } from '../ui';
import { forgetLayout, loadLayout, saveLayout } from './layout';
import { TILES, TILE_KINDS, type TileKind } from './tiles';

// dockview's own "spaced" theme gives separate rounded tiles with gaps between them;
// styles.css recolours it from the app's tokens, so presets and light/dark carry over.
const theme: DockviewTheme = { ...themeLightSpaced, name: 'vkore', gap: 10 };

function frame(kind: TileKind): FC<IDockviewPanelProps<{ name: string }>> {
  const { Body, flush } = TILES[kind];
  const Tile: FC<IDockviewPanelProps<{ name: string }>> = ({ params }) => (
    <div className={`tile-body ${flush ? 'flush' : ''}`}>
      <Body name={params.name} />
    </div>
  );
  return Tile;
}

// One stable component per tile kind: dockview looks a panel's component up by name.
const components = Object.fromEntries(TILE_KINDS.map((kind) => [kind, frame(kind)])) as Record<TileKind, FC<IDockviewPanelProps<{ name: string }>>>;

/** Buttons at the right of every tile's header: expand to fill the workspace, and close. */
const HeaderActions: FC<IDockviewHeaderActionsProps> = ({ api, activePanel }) => {
  const [, redraw] = useState(0);
  const maximized = api.isMaximized();
  return (
    <div className="tile-actions">
      <button
        className="icon"
        aria-label={maximized ? 'Restore tile' : 'Expand tile'}
        title={maximized ? 'Restore tile' : 'Expand tile'}
        onClick={() => {
          if (maximized) api.exitMaximized();
          else api.maximize();
          redraw((count) => count + 1);
        }}
      >
        {maximized ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
      </button>
      <button className="icon" aria-label="Close tile" title="Close tile" disabled={!activePanel} onClick={() => activePanel?.api.close()}>
        <X size={14} />
      </button>
    </div>
  );
};

function defaultLayout(api: DockviewApi, name: string) {
  api.clear();
  const params = { name };
  const add = (kind: TileKind, position?: Parameters<DockviewApi['addPanel']>[0]['position'], size?: { initialWidth?: number; initialHeight?: number }) =>
    api.addPanel({ id: kind, component: kind, title: TILES[kind].title, params, position, ...size });
  add('console');
  add('boot', { referencePanel: 'console', direction: 'right' }, { initialWidth: 400 });
  add('events', { referencePanel: 'console', direction: 'below' }, { initialHeight: 210 });
  add('resources', { referencePanel: 'boot', direction: 'below' });
  add('configuration', { referencePanel: 'resources', direction: 'within' });
  api.getPanel('console')?.api.setActive();
}

/** A machine's workspace: its tiles, which can be dragged to dock beside or on top of each other, resized, expanded and closed. */
export function Workspace({ machine }: { machine: Machine }) {
  const { name } = machine;
  const dock = useRef<DockviewApi | null>(null);
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [menu, setMenu] = useState(false);
  const bar = useRef<HTMLDivElement>(null);

  const onReady = (event: DockviewReadyEvent) => {
    const api = event.api;
    dock.current = api;
    const saved = loadLayout(name);
    let restored = false;
    if (saved) {
      try {
        api.fromJSON(saved as Parameters<DockviewApi['fromJSON']>[0]);
        restored = true;
      } catch {
        // A layout this version cannot read: fall back to the default one.
      }
    }
    if (!restored) defaultLayout(api, name);
    setOpen(new Set(api.panels.map((panel) => panel.id)));
    let timer: ReturnType<typeof setTimeout> | undefined;
    api.onDidLayoutChange(() => {
      setOpen(new Set(api.panels.map((panel) => panel.id)));
      clearTimeout(timer);
      timer = setTimeout(() => saveLayout(name, api.toJSON()), 300);
    });
  };

  // Close the add-tile menu on an outside click or Escape.
  useEffect(() => {
    if (!menu) return;
    const close = (event: MouseEvent | KeyboardEvent) => {
      if (event instanceof KeyboardEvent) {
        if (event.key === 'Escape') setMenu(false);
      } else if (!bar.current?.contains(event.target as Node)) {
        setMenu(false);
      }
    };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', close);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', close);
    };
  }, [menu]);

  const show = (kind: TileKind) => {
    const api = dock.current;
    if (!api) return;
    const existing = api.getPanel(kind);
    if (existing) {
      existing.api.setActive();
    } else {
      const anchor = api.activePanel ?? api.panels[0];
      api.addPanel({
        id: kind,
        component: kind,
        title: TILES[kind].title,
        params: { name },
        // A tab in the active tile, so a new tile never squeezes the others; drag it out to dock it elsewhere.
        position: anchor ? { referencePanel: anchor, direction: 'within' } : undefined,
      });
    }
    setMenu(false);
  };

  const reset = () => {
    const api = dock.current;
    if (!api) return;
    forgetLayout(name);
    defaultLayout(api, name);
  };

  const available = useMemo(() => TILE_KINDS.filter((kind) => !open.has(kind)).length, [open]);

  return (
    <div className="workspace">
      <div className="ws-bar" ref={bar}>
        <div className="ws-title">
          <span className="plate"><LayoutPanelLeft size={18} /></span>
          <h1>{name}</h1>
          <Pill phase={machine.run.phase} />
        </div>
        <div className="ws-actions">
          <div className="menu-anchor">
            <button onClick={() => setMenu(!menu)} aria-haspopup="menu" aria-expanded={menu}>
              <Plus size={15} /> Tiles{available > 0 ? ` · ${available} closed` : ''}
            </button>
            {menu && (
              <ul className="menu card" role="menu" aria-label="Tiles">
                {TILE_KINDS.map((kind) => {
                  const { Icon, title } = TILES[kind];
                  return (
                    <li key={kind} role="none">
                      <button role="menuitem" className="menu-item" onClick={() => show(kind)}>
                        <Icon size={16} />
                        <span>{title}</span>
                        {open.has(kind) && <Check size={15} className="faint" />}
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
          <button onClick={reset} title="Restore the default layout"><RotateCcw size={15} /> Reset layout</button>
          <StartStop machine={machine} />
        </div>
      </div>
      {machine.run.phase === 'failed' && machine.run.message && <p className="notice bad ws-notice">{machine.run.message}</p>}
      <div className="ws-dock vk-dock">
        <DockviewReact components={components} theme={theme} rightHeaderActionsComponent={HeaderActions} onReady={onReady} />
      </div>
    </div>
  );
}

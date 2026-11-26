import { Boxes, HardDrive, LayoutDashboard, Moon, PanelLeftClose, PanelLeftOpen, Sun } from 'lucide-react';
import { NavLink } from 'react-router-dom';
import type { Phase } from './api';
import { MpMark, type MarkMotion } from './MpMark';
import { useStore } from './store';
import { ThemePicker } from './themes/ThemePicker';
import type { Theme } from './theme';
import { isActive } from './ui';

const DOT: Record<Phase, string> = {
  idle: 'quiet',
  starting: 'warm',
  booting: 'warm',
  ready: 'good',
  degraded: 'warn',
  stopping: 'warm',
  failed: 'bad',
};

export function Sidebar({ mark, theme, onTheme, preset, onPreset, collapsed, onCollapse }: {
  mark: MarkMotion;
  collapsed: boolean;
  onCollapse: (next: boolean) => void;
  theme: Theme;
  onTheme: (next: Theme) => void;
  preset: string;
  onPreset: (id: string) => void;
}) {
  const { machines, host } = useStore();
  const next: Theme = theme === 'dark' ? 'light' : 'dark';
  return (
    <aside className="sidebar" aria-label="Sidebar">
      <div className="side-brand">
        <MpMark size={36} motion={mark} />
        <span className="side-brand-text">
          <span className="logo">vKore</span>
          <span className="context">experimental</span>
        </span>
      </div>
      <button
        className="icon side-toggle"
        onClick={() => onCollapse(!collapsed)}
        aria-label={collapsed ? 'Expand the sidebar' : 'Collapse the sidebar'}
        aria-expanded={!collapsed}
        title={collapsed ? 'Expand the sidebar' : 'Collapse the sidebar'}
      >
        {collapsed ? <PanelLeftOpen size={17} /> : <PanelLeftClose size={17} />}
      </button>

      <nav className="side-nav" aria-label="Main">
        <NavLink to="/" end title="Overview"><LayoutDashboard size={18} /><span className="label">Overview</span></NavLink>
        <NavLink to="/machines" end title="Machines"><Boxes size={18} /><span className="label">Machines</span></NavLink>
        <NavLink to="/volumes" title="Volumes"><HardDrive size={18} /><span className="label">Volumes</span></NavLink>
      </nav>

      <div className="side-section">
        <p className="side-heading label">Workspaces</p>
        <nav className="side-machines" aria-label="Machine workspaces">
          {(machines ?? []).map((machine) => (
            <NavLink key={machine.name} to={`/machines/${encodeURIComponent(machine.name)}`} title={`${machine.name}`}>
              <span className={`dot ${DOT[machine.run.phase]} ${isActive(machine.run.phase) && machine.run.phase !== 'ready' ? 'pulse' : ''}`} aria-hidden />
              <span className="label">{machine.name}</span>
            </NavLink>
          ))}
          {machines?.length === 0 && <span className="faint small label">No machines yet</span>}
        </nav>
      </div>

      <div className="side-foot">
        {host && <span className="chip label">{host.running} running</span>}
        <div className="side-tools">
          <ThemePicker value={preset} onChange={onPreset} placement="up" />
          <button className="icon" onClick={() => onTheme(next)} aria-label={`Switch to ${next} theme`} title={`Switch to ${next} theme`}>
            {theme === 'dark' ? <Sun size={16} /> : <Moon size={16} />}
          </button>
        </div>
      </div>
    </aside>
  );
}

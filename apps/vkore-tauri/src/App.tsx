import { Boxes, LayoutDashboard, Moon, Sun } from 'lucide-react';
import { useEffect, useState } from 'react';
import { NavLink, Navigate, Route, Routes } from 'react-router-dom';
import { MpMark } from './MpMark';
import { Dashboard } from './pages/Dashboard';
import { MachinePage } from './pages/MachinePage';
import { Machines } from './pages/Machines';
import { StoreProvider, useStore } from './store';
import { applyTheme, preferredTheme, saveTheme, type Theme } from './theme';
import { applyPreset, preferredPreset, savePreset } from './themes/apply';
import { ThemePicker } from './themes/ThemePicker';

function Brand({ busy }: { busy: boolean }) {
  // The mark boots once as the window opens, then shows the host's activity.
  const [intro, setIntro] = useState(true);
  useEffect(() => {
    const done = setTimeout(() => setIntro(false), 2700);
    return () => clearTimeout(done);
  }, []);
  return (
    <span className="logo">
      <MpMark size={36} motion={intro ? 'intro' : busy ? 'busy' : 'still'} />
      vKore
    </span>
  );
}

function Shell() {
  const { error, host, machines } = useStore();
  const booting = machines?.some((m) => ['starting', 'booting', 'stopping'].includes(m.run.phase)) ?? false;
  const [theme, setTheme] = useState<Theme>(preferredTheme);
  const [preset, setPreset] = useState(preferredPreset);
  useEffect(() => {
    applyTheme(theme);
    applyPreset(preset, theme);
  }, [theme, preset]);
  const next: Theme = theme === 'dark' ? 'light' : 'dark';

  return (
    <div className="shell">
      <header className="topbar">
        <div className="brand">
          <Brand busy={booting} />
          <span className="context">experimental</span>
        </div>
        <nav className="nav" aria-label="Main">
          <NavLink to="/" end><LayoutDashboard size={16} /> Overview</NavLink>
          <NavLink to="/machines"><Boxes size={16} /> Machines</NavLink>
        </nav>
        <div className="topbar-right">
          {host && <span className="chip">{host.running} running</span>}
          <ThemePicker
            value={preset}
            onChange={(id) => {
              savePreset(id);
              setPreset(id);
            }}
          />
          <button className="icon" onClick={() => {
              saveTheme(next);
              setTheme(next);
            }} aria-label={`Switch to ${next} theme`} title={`Switch to ${next} theme`}>
            {theme === 'dark' ? <Sun size={16} /> : <Moon size={16} />}
          </button>
        </div>
      </header>
      {error && <p className="notice bad" role="alert">{error}</p>}
      <main>
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/machines" element={<Machines />} />
          <Route path="/machines/:name" element={<MachinePage />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </main>
    </div>
  );
}

export function App() {
  return (
    <StoreProvider>
      <Shell />
    </StoreProvider>
  );
}

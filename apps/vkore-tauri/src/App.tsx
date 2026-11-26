import { useEffect, useState } from 'react';
import { Navigate, Route, Routes, useLocation, useMatch } from 'react-router-dom';
import { ErrorBoundary } from './ErrorBoundary';
import { Dashboard } from './pages/Dashboard';
import { Hardware } from './pages/Hardware';
import { MachinePage } from './pages/MachinePage';
import { Machines } from './pages/Machines';
import { Volumes } from './pages/Volumes';
import { Sidebar } from './Sidebar';
import { StoreProvider, useStore } from './store';
import { applyTheme, preferredTheme, saveTheme, type Theme } from './theme';
import { applyPreset, preferredPreset, savePreset } from './themes/apply';

const SIDEBAR_KEY = 'vkore.sidebar';

function savedCollapsed(): boolean {
  try {
    return localStorage.getItem(SIDEBAR_KEY) === 'collapsed';
  } catch {
    return false;
  }
}

function Shell() {
  const { error, machines } = useStore();
  const [theme, setTheme] = useState<Theme>(preferredTheme);
  const [preset, setPreset] = useState(preferredPreset);
  const [collapsed, setCollapsed] = useState(savedCollapsed);
  useEffect(() => {
    applyTheme(theme);
    applyPreset(preset, theme);
  }, [theme, preset]);
  const location = useLocation();
  // A machine's workspace fills the window; every other page is a centred column.
  const workspace = !!useMatch('/machines/:name');

  // The mark boots once as the window opens, then shows the host's activity.
  const [intro, setIntro] = useState(true);
  useEffect(() => {
    const done = setTimeout(() => setIntro(false), 2700);
    return () => clearTimeout(done);
  }, []);
  const booting = machines?.some((m) => ['starting', 'booting', 'stopping'].includes(m.run.phase)) ?? false;

  return (
    <div className={`app ${collapsed ? 'collapsed' : ''}`}>
      <Sidebar
        collapsed={collapsed}
        onCollapse={(next) => {
          try {
            localStorage.setItem(SIDEBAR_KEY, next ? 'collapsed' : 'open');
          } catch {
            // Not persisted; still applied for this session.
          }
          setCollapsed(next);
        }}
        mark={intro ? 'intro' : booting ? 'busy' : 'still'}
        theme={theme}
        onTheme={(next) => {
          saveTheme(next);
          setTheme(next);
        }}
        preset={preset}
        onPreset={(id) => {
          savePreset(id);
          setPreset(id);
        }}
      />
      <main className={`main ${workspace ? 'fill' : ''}`}>
        {error && <p className="notice bad page-pad" role="alert">{error}</p>}
        <ErrorBoundary resetKey={location.pathname}>
          <div className={workspace ? 'fill-page' : 'page'}>
            <Routes>
              <Route path="/" element={<Dashboard />} />
              <Route path="/machines" element={<Machines />} />
              <Route path="/machines/:name" element={<MachinePage />} />
              <Route path="/machines/:name/hardware" element={<Hardware />} />
              <Route path="/volumes" element={<Volumes />} />
              <Route path="/volumes/:name" element={<Volumes />} />
              <Route path="*" element={<Navigate to="/" replace />} />
            </Routes>
          </div>
        </ErrorBoundary>
      </main>
    </div>
  );
}

export function App() {
  return (
    <ErrorBoundary>
      <StoreProvider>
        <Shell />
      </StoreProvider>
    </ErrorBoundary>
  );
}

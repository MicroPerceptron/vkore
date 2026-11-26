import '@fontsource-variable/fraunces/wght.css';
import '@fontsource-variable/fraunces/wght-italic.css';
import '@fontsource-variable/outfit/index.css';
import React from 'react';
import { createRoot } from 'react-dom/client';
import { HashRouter } from 'react-router-dom';
import { App } from './App';
import './styles.css';
import { applyTheme, preferredTheme } from './theme';
import { applyPreset, preferredPreset } from './themes/apply';

// Set the theme before the first paint so a dark window never flashes light.
const mode = preferredTheme();
applyTheme(mode);
applyPreset(preferredPreset(), mode);

createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <HashRouter>
      <App />
    </HashRouter>
  </React.StrictMode>,
);

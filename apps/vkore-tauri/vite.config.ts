import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

// The CSP serves fonts and images from 'self' only, so nothing is inlined as a
// data: URI, and the dev server is pinned to the port tauri.conf.json names.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: '127.0.0.1' },
  build: { target: 'es2022', assetsInlineLimit: 0, sourcemap: false },
  test: { environment: 'node' },
});

import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Tauri sets these while running `tauri dev` / `tauri build`.
const host = process.env.TAURI_DEV_HOST;
const tauriDebug = !!process.env.TAURI_ENV_DEBUG;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: { ignored: ['**/src-tauri/**', '**/target/**', '**/fixtures/**'] },
    // In browser development the UI talks to the local bridge
    // (`npm run dev:web` starts it); the desktop app uses Tauri IPC instead.
    proxy: { '/api': { target: 'http://127.0.0.1:4317', changeOrigin: false } },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    // WebView2 on Windows is Chromium; macOS 13+ ships WebKit 16+.
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari16',
    minify: tauriDebug ? false : 'esbuild',
    sourcemap: tauriDebug,
    outDir: 'dist',
    emptyOutDir: true,
    chunkSizeWarningLimit: 1200,
  },
});

import { defineConfig } from 'vite';

// https://vitejs.dev/config/
export default defineConfig({
  // Prevent Vite from obscuring Rust errors
  clearScreen: false,
  server: {
    port: 5173,
    // Tauri expects a fixed port; fail if it is not available
    strictPort: true,
    watch: {
      // Tell Vite to ignore watching `src-tauri`
      ignored: ['**/src-tauri/**'],
    },
  },
  // Env vars prefixed with TAURI_ are added by the Tauri CLI
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    // Tauri uses Chromium on Windows and WebKit on macOS/Linux
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    // do not minify for debug builds
    minify: !process.env.TAURI_ENV_DEBUG ? 'esbuild' : false,
    // produce sourcemaps for debug builds
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});

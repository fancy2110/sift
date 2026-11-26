import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';

const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [tailwindcss(), svelte()],
  clearScreen: false,
  server: {
    port: Number(process.env.VITE_PORT ?? 5175),
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: 'ws',
          host,
          port: Number(process.env.VITE_HMR_PORT ?? 5176)
        }
      : undefined,
    watch: {
      // Don't watch the Rust backend.
      ignored: ['**/src-tauri/**']
    }
  },
  preview: {
    port: Number(process.env.VITE_PREVIEW_PORT ?? 5178),
    strictPort: true
  },
  // Produce reasonable bundles across platforms.
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    target:
      process.env.TAURI_ENV_PLATFORM === 'windows'
        ? 'chrome105'
        : 'safari13',
    minify: !process.env.TAURI_ENV_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG
  }
});

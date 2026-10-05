/**
 * Whether the page runs inside the Tauri webview with the native command
 * bridge present.
 *
 * Opening the Vite dev server in a plain browser (`pnpm dev`, then visiting the
 * URL outside the shell) leaves `window.__TAURI_INTERNALS__` undefined; in that
 * mode every command routes to the in-process mock backend
 * (`src/lib/mock/backend.ts`) instead of failing.
 */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/**
 * Keep the native window chrome (title bar / traffic lights / controls) in
 * step with the webview's color scheme.
 *
 * The document palette is driven entirely by CSS `prefers-color-scheme`, but
 * the native title bar does not read that: the window keeps whatever theme it
 * resolved at launch until it is told otherwise. Without this sync the title
 * bar stays light while the content renders dark (or vice versa) when the OS
 * appearance changes or the app launches in an appearance the window did not
 * initially resolve to.
 *
 * The window theme is set to the resolved light/dark value and updated live;
 * `setTheme(null)` would merely mean "follow system", which is the default and
 * does not always re-apply to the chrome after a change in alpha builds.
 */
import { getCurrentWindow } from '@tauri-apps/api/window';
import { isTauri } from './runtime';

export async function syncWindowTheme(): Promise<void> {
  if (!isTauri()) return;
  const appWindow = getCurrentWindow();

  const apply = () => {
    const theme = window.matchMedia('(prefers-color-scheme: dark)').matches
      ? 'dark'
      : 'light';
    // Cosmetic only; never let a failed IPC call affect the running app.
    void appWindow.setTheme(theme).catch(() => {});
  };

  apply();

  // Track the OS appearance directly: the media query updates in the webview
  // even if a native theme-change event is dropped in an alpha build, and it
  // is also the exact signal the CSS palette follows, so the two can never
  // diverge.
  window
    .matchMedia('(prefers-color-scheme: dark)')
    .addEventListener('change', apply);

  try {
    await appWindow.onThemeChanged(apply);
  } catch {
    // If listening is unsupported the media-query listener still covers it.
  }
}

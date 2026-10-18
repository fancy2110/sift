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
 * The window (and its `@tauri-apps/api` module) is loaded **dynamically** on
 * purpose. A static import pulled the window API into the initial chunk
 * (143 KB vs 129 KB) for a purely cosmetic follow-up: the window already
 * starts with the system theme, so this only corrects an edge case and can
 * happen a tick after the first paint without the user noticing. Startup is
 * the one budget the product treats as a hard requirement (see
 * docs/perf/startup.md), so the initial chunk keeps the smaller form.
 *
 * `setTheme(null)` would merely mean "follow system", which is the default and
 * does not always re-apply to the chrome after a change in alpha builds; the
 * concrete light/dark value is applied instead and kept updated live.
 */
import { isTauri } from './runtime';

export async function syncWindowTheme(): Promise<void> {
  if (!isTauri()) return;

  let wired = false;
  const applyTheme = async () => {
    // Cosmetic only; never let a failed import or IPC call affect the app.
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      const appWindow = getCurrentWindow();
      if (!wired) {
        wired = true;
        // The media listener below is the primary signal; this native event
        // is a fallback for a webview that does not repaint the media query.
        try {
          await appWindow.onThemeChanged(() => void applyTheme());
        } catch {
          // Not supported: the media-query listener still covers changes.
        }
      }
      const theme = window.matchMedia('(prefers-color-scheme: dark)').matches
        ? 'dark'
        : 'light';
      await appWindow.setTheme(theme);
    } catch {
      // Ignore: the content palette is unaffected.
    }
  };

  // Track the OS appearance directly: the media query updates in the webview
  // even if a native theme-change event is dropped in an alpha build, and it
  // is also the exact signal the CSS palette follows, so the two can never
  // diverge.
  window
    .matchMedia('(prefers-color-scheme: dark)')
    .addEventListener('change', () => void applyTheme());

  await applyTheme();
}

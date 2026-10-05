/**
 * Directory picker with the same contract across the Tauri shell and browser
 * mode. Tauri opens NSOpenPanel; in the browser there is no native panel and
 * no absolute-path picker, so the requested default path is treated as
 * granted — enough to exercise the permission grant flow against the mock.
 */
import { open as tauriOpen } from '@tauri-apps/plugin-dialog';
import { isTauri } from './runtime';

export async function pickDirectory(defaultPath: string): Promise<string | null> {
  if (!isTauri()) return defaultPath;
  const selected = await tauriOpen({ directory: true, multiple: false, defaultPath });
  if (!selected) return null;
  return Array.isArray(selected) ? (selected[0] ?? null) : selected;
}

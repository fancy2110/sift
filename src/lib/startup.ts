/**
 * Startup phase marks, reported to the native trace.
 *
 * Deliberately dependency-free (no `./ipc`, which drags the mock backend into
 * the initial chunk): a Tauri command invoke is one direct call. Marks are
 * fire-and-forget and only collected when the native side instruments them.
 */
import { invoke } from '@tauri-apps/api/core';

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

export function startupMark(label: string): void {
  if (!('__TAURI_INTERNALS__' in window)) return;
  try {
    void invoke('startup_mark', { label, wallMs: Date.now() });
  } catch {
    // Tracing must never affect startup.
  }
}

/**
 * True only for the first HomeView mount of this process. Intro animations are
 * skipped then: after a cold launch the user waits for content, not for a
 * 600 ms staggered fly-in. Later navigations (back from another view) keep the
 * animation as a navigation affordance.
 */
export const firstBoot: { value: boolean } = { value: true };

/** Mark first-contentful-paint and the first animation frame. */
export function observeFirstPaint(): void {
  try {
    const observer = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        startupMark(`paint:${entry.name} (${Math.round(entry.startTime)} ms)`);
      }
    });
    observer.observe({ type: 'paint', buffered: true });
  } catch {
    // Not all webviews support the paint timing API.
  }
  requestAnimationFrame(() =>
    requestAnimationFrame(() => startupMark('paint:first-frame')),
  );
}

/** Fetch the combined native + webview trace (handy from devtools). */
export async function readStartupTrace(): Promise<
  { label: string; sinceOriginMs: number; wallMs: number | null }[]
> {
  if (!('__TAURI_INTERNALS__' in window)) return [];
  return invoke('startup_trace');
}

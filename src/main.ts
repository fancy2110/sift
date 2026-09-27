import { mount } from 'svelte';
import { invoke } from '@tauri-apps/api/core';
import './app.css';
import App from './App.svelte';

const app = mount(App, { target: document.getElementById('app')! });

// TEMP DIAG: report pointer interactions to the native backend, which mirrors
// the message into the native window title (readable via AXTitle) and a log
// file, so we can see whether clicks reach the webview and hit the right node.
function describe(t: EventTarget | null): string {
  const el = t as HTMLElement | null;
  if (!el || el.nodeType !== 1) return String(t);
  return [
    el.tagName,
    el.id ? '#' + el.id : '',
    el.getAttribute('data-od-id') ? '[od=' + el.getAttribute('data-od-id') + ']' : '',
    el.getAttribute('role') ? '[role=' + el.getAttribute('role') + ']' : '',
    el.hasAttribute('data-tauri-drag-region') ? '[DRAG]' : '',
    'cls=' + (el.className && typeof el.className === 'string' ? el.className.slice(0, 40) : '')
  ].join('');
}
function diag(s: string) {
  invoke('diag_log', { message: s }).catch(() => undefined);
}
diag('MAIN-LOADED diag-channel-ready');
for (const type of ['pointerdown', 'mousedown', 'click'] as const) {
  document.addEventListener(type, (e) => diag(type + ' -> ' + describe(e.target)), true);
}

// TEMP DIAG: hit-test audit — report what elements stack on top of the
// titlebar buttons and the bottom status bar at their center points.
function audit() {
  for (const od of ['auto-toggle', 'scan-button', 'ai-summary']) {
    const el = document.querySelector<HTMLElement>('[data-od-id="' + od + '"]');
    if (!el) {
      diag('AUDIT ' + od + ': NOT FOUND');
      continue;
    }
    const r = el.getBoundingClientRect();
    const cx = r.left + r.width / 2;
    const cy = r.top + r.height / 2;
    const stack = document
      .elementsFromPoint(cx, cy)
      .slice(0, 6)
      .map((n) => describe(n))
      .join(' || ');
    diag(
      'AUDIT ' +
        od +
        ' center=' +
        cx.toFixed(0) +
        ',' +
        cy.toFixed(0) +
        ' stack: ' +
        stack
    );
  }
}
setTimeout(audit, 1500);

export default app;

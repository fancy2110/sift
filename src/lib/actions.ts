/**
 * Close a popover when the pointer goes down anywhere outside `node`.
 * Capture phase, so clicks on nested buttons that stop propagation still
 * close the popover first.
 */
export function clickOutside(node: HTMLElement, onOutside: () => void) {
  const handler = (event: Event) => {
    if (!node.contains(event.target as Node)) onOutside();
  };
  // Both: pointerdown feels immediate, click covers keyboard/synthetic
  // activations that never fire a pointer event.
  document.addEventListener('pointerdown', handler, true);
  document.addEventListener('click', handler, true);
  return {
    destroy() {
      document.removeEventListener('pointerdown', handler, true);
      document.removeEventListener('click', handler, true);
    }
  };
}

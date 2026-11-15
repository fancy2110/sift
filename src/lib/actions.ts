/**
 * Close a popover when the pointer goes down anywhere outside `node`.
 * Capture phase, so clicks on nested buttons that stop propagation still
 * close the popover first.
 */
export function clickOutside(node: HTMLElement, onOutside: () => void) {
  const handler = (event: Event) => {
    if (!node.contains(event.target as Node)) onOutside();
  };
  // Pointerdown only: closing on capture-phase `click` can remove the menu
  // before the item's own handler runs (the node may already be detached by a
  // re-render), which silently swallowed menu actions such as 编辑.
  document.addEventListener('pointerdown', handler, true);
  return {
    destroy() {
      document.removeEventListener('pointerdown', handler, true);
    }
  };
}

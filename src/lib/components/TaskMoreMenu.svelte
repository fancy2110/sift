<script lang="ts">
  /**
   * The "more" menu shared by every scheduled-task row (routines and cleanup
   * rules). One render path means the two row kinds can never drift apart
   * again: the item set, order, icons and disabled gating are defined here
   * exactly once — 立即执行 / 编辑 / 删除, with 删除 kept in the danger style.
   */
  import Icon from './Icon.svelte';
  import { clickOutside } from '../actions';
  import { t } from '../i18n.svelte';

  let {
    entityTitle,
    enabled,
    onRun,
    onEdit,
    onDelete
  }: {
    entityTitle: string;
    /** Disables "run now" for both row kinds, matching the backend gate. */
    enabled: boolean;
    onRun: () => void;
    onEdit: () => void;
    onDelete: () => void;
  } = $props();

  let open = $state(false);
  let menuEl = $state<HTMLDivElement | null>(null);

  function close() {
    open = false;
  }

  function choose(action: () => void) {
    open = false;
    action();
  }

  // Move focus into the popover while it is open so keyboard/AT users land on
  // the menu instead of losing it behind the trigger.
  $effect(() => {
    if (open) menuEl?.focus({ preventScroll: true });
  });

  // Escape dismisses the popover; the menu is otherwise pointer-only.
  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.stopPropagation();
      close();
    }
  }
</script>

<div class="rt-more" use:clickOutside={close}>
  <button
    type="button"
    class="rt-btn"
    aria-haspopup="menu"
    aria-expanded={open}
    aria-label={t('routines.moreAria', [entityTitle])}
    title={t('routines.more')}
    onclick={() => (open = !open)}
  >
    <Icon name="sliders" size={12} /> {t('routines.more')}
  </button>
  {#if open}
    <div class="rt-menu" role="menu" tabindex="-1" bind:this={menuEl} onkeydown={onKeydown}>
      <button
        type="button"
        role="menuitem"
        class="rt-menu-item"
        disabled={!enabled}
        aria-disabled={!enabled}
        onclick={() => choose(onRun)}
      >
        <Icon name="play" size={11} /> {t('routines.runNow')}
      </button>
      <button type="button" role="menuitem" class="rt-menu-item" onclick={() => choose(onEdit)}>
        <Icon name="sliders" size={11} /> {t('settings.edit')}
      </button>
      <button
        type="button"
        role="menuitem"
        class="rt-menu-item rt-menu-danger"
        onclick={() => choose(onDelete)}
      >
        <Icon name="trash" size={11} /> {t('settings.delete')}
      </button>
    </div>
  {/if}
</div>

<style>
  .rt-more {
    position: relative;
  }
  .rt-menu {
    position: absolute;
    right: 0;
    top: calc(100% + 6px);
    z-index: 20;
    min-width: 150px;
    padding: 5px;
    border-radius: 10px;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
    box-shadow: 0 14px 30px -14px rgb(0 0 0 / 0.45);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .rt-menu-item {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 6px 8px;
    border: none;
    border-radius: 7px;
    background: transparent;
    color: var(--color-fg);
    font-size: 12px;
    text-align: left;
    cursor: pointer;
  }
  .rt-menu-item:hover:not(:disabled) {
    background: color-mix(in oklch, var(--color-surface-2) 70%, transparent);
  }
  .rt-menu-item:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .rt-menu-danger {
    color: var(--color-danger);
  }
  .rt-btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 27px;
    padding: 0 10px;
    border-radius: 8px;
    border: 1px solid var(--color-border-strong);
    background: transparent;
    color: var(--color-muted);
    font-size: 11px;
    font-weight: 560;
    cursor: default;
    white-space: nowrap;
  }
  .rt-btn:hover {
    color: var(--color-fg);
    border-color: var(--color-faint);
  }
</style>

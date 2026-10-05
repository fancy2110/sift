<script lang="ts">
  import { store } from '../store.svelte';
  import { t } from '../i18n.svelte';
  import Icon from './Icon.svelte';
  import { fly } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';

  // TCC requests first (user can grant immediately), POSIX/admin after.
  let sorted = $derived([...store.permissionRequests].sort(
    (a, b) => Number(b.tcc) - Number(a.tcc)));

  // Floating, collapsible panel on the right edge so the permission list never
  // reflows the home layout. Auto-expands the first time requests appear; once
  // the user collapses it manually, later arrivals leave it collapsed (the tab
  // badge still updates). Reset when the queue drains.
  let open = $state(false);
  let userCollapsed = $state(false);
  let prevCount = 0;
  $effect(() => {
    const n = store.permissionRequests.length;
    if (n === 0) {
      userCollapsed = false;
      open = false;
    } else if (prevCount === 0 && !userCollapsed) {
      open = true;
    }
    prevCount = n;
  });

  function toggle() {
    open = !open;
    if (!open) userCollapsed = true;
  }
</script>

{#if sorted.length}
  {#if !open}
    <button
      type="button"
      class="drawer-tab"
      onclick={toggle}
      in:fly={{ x: 80, duration: 260, easing: cubicOut }}
    >
      <span class="tab-badge">{sorted.length}</span>
      <Icon name="shield" size={14} />
      <span class="tab-text">{t('permission.drawerTab')}</span>
      <Icon name="chevronLeft" size={13} />
    </button>
  {/if}

  {#if open}
    <aside
      class="drawer"
      in:fly={{ x: 380, duration: 300, easing: cubicOut }}
      out:fly={{ x: 380, duration: 220, easing: cubicOut }}
    >
      <header class="drawer-head">
        <span>{t('permission.heading', [sorted.length])}</span>
        <div class="head-actions">
          <button
            type="button"
            class="head-btn"
            onclick={() => store.skipAllPermissions()}
          >
            {t('permission.skipAll')}
          </button>
          <button
            type="button"
            class="head-icon"
            aria-label={t('permission.collapse')}
            onclick={toggle}
          >
            <Icon name="chevronRight" size={15} />
          </button>
        </div>
      </header>

      <div class="drawer-scroll">
        {#each sorted as req (req.id)}
          <div class="perm-card" class:posix={!req.tcc}>
            <p class="perm-title">
              <span class="perm-tag" class:tag-posix={!req.tcc}>
                {req.tcc ? t('permission.tccTag') : t('permission.adminTag')}
              </span>
              {req.name}
            </p>
            <p class="perm-msg">
              {req.tcc ? t('permission.message', [req.name]) : t('permission.adminNeeded')}
            </p>
            <div class="perm-actions">
              {#if req.tcc}
                <button
                  type="button"
                  class="perm-btn primary"
                  onclick={() => store.grantPermission(req)}
                >
                  {t('permission.grant')}
                </button>
              {/if}
              <button
                type="button"
                class="perm-btn"
                onclick={() => store.skipPermission(req)}
              >
                {t('permission.skip')}
              </button>
            </div>
          </div>
        {/each}
      </div>
    </aside>
  {/if}
{/if}

<style>
  .drawer-tab {
    position: fixed;
    top: 104px;
    right: 0;
    z-index: 60;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 9px 12px 9px 10px;
    border-radius: 12px 0 0 12px;
    border: 1px solid color-mix(in oklch, var(--color-border-strong) 60%, transparent);
    border-right: none;
    background: color-mix(in oklch, var(--color-surface) 94%, var(--color-shadow) 4%);
    color: var(--color-fg);
    box-shadow: -14px 22px 44px -26px var(--color-shadow);
    cursor: default;
    transition: background 0.15s ease;
  }
  .drawer-tab:hover {
    background: color-mix(in oklch, var(--color-surface-3, var(--color-surface-2)) 80%, transparent);
  }
  .tab-badge {
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    border-radius: 9px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: var(--color-violet);
    color: #fff;
    font-size: 10.5px;
    font-weight: 700;
  }
  .tab-text {
    font-family: var(--font-mono);
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.08em;
  }

  .drawer {
    position: fixed;
    top: 104px;
    right: 14px;
    bottom: 30px;
    width: 326px;
    max-width: calc(100vw - 28px);
    z-index: 60;
    display: flex;
    flex-direction: column;
    border-radius: 16px;
    border: 1px solid color-mix(in oklch, var(--color-border-strong) 55%, transparent);
    background: color-mix(in oklch, var(--color-surface) 96%, var(--color-shadow) 3%);
    box-shadow: 0 34px 80px -28px var(--color-shadow);
    overflow: hidden;
  }
  .drawer-head {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 14px 14px 12px 16px;
    font-family: var(--font-mono);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.12em;
    color: var(--color-faint);
    border-bottom: 1px solid color-mix(in oklch, var(--color-border-strong) 36%, transparent);
  }
  .head-actions {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .head-btn {
    padding: 4px 9px;
    border-radius: 8px;
    border: 1px solid color-mix(in oklch, var(--color-border-strong) 60%, transparent);
    background: transparent;
    color: var(--color-faint);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.08em;
    cursor: default;
    transition: background 0.15s ease, color 0.15s ease;
  }
  .head-btn:hover {
    background: color-mix(in oklch, var(--color-surface-3, var(--color-surface-2)) 70%, transparent);
    color: var(--color-fg);
  }
  .head-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--color-faint);
    cursor: default;
    transition: background 0.15s ease, color 0.15s ease;
  }
  .head-icon:hover {
    background: color-mix(in oklch, var(--color-surface-3, var(--color-surface-2)) 70%, transparent);
    color: var(--color-fg);
  }

  .drawer-scroll {
    flex: 1 1 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-width: thin;
    scrollbar-color:
      color-mix(in oklch, var(--color-border-strong) 80%, transparent)
      transparent;
  }
  .drawer-scroll::-webkit-scrollbar {
    width: 8px;
  }
  .drawer-scroll::-webkit-scrollbar-track {
    background: transparent;
  }
  .drawer-scroll::-webkit-scrollbar-thumb {
    border-radius: 8px;
    background: color-mix(in oklch, var(--color-border-strong) 70%, transparent);
  }

  .perm-card {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 14px;
    border-radius: 14px;
    border: 1px solid color-mix(in oklch, var(--color-border-strong) 55%, transparent);
    background: color-mix(in oklch, var(--color-surface) 92%, var(--color-shadow) 6%);
    box-shadow: 0 16px 36px -24px var(--color-shadow);
  }
  .perm-card.posix {
    border-color: color-mix(in oklch, var(--color-amber, #c8852c) 38%, transparent);
  }
  .perm-title {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    font-family: var(--font-mono);
    font-size: 11.5px;
    font-weight: 600;
    color: var(--color-fg);
  }
  .perm-tag {
    flex-shrink: 0;
    padding: 2px 8px;
    border-radius: 7px;
    font-size: 9.5px;
    letter-spacing: 0.12em;
    color: var(--color-violet);
    background: color-mix(in oklch, var(--color-violet) 14%, transparent);
  }
  .perm-tag.tag-posix {
    color: var(--color-amber, #c8852c);
    background: color-mix(in oklch, var(--color-amber, #c8852c) 14%, transparent);
  }
  .perm-msg {
    margin: 0;
    font-size: 12.5px;
    color: var(--color-faint);
  }
  .perm-actions {
    display: flex;
    gap: 8px;
    align-self: flex-end;
  }
  .perm-btn {
    padding: 7px 14px;
    border-radius: 10px;
    border: 1px solid color-mix(in oklch, var(--color-border-strong) 60%, transparent);
    background: transparent;
    color: var(--color-fg);
    font-size: 12px;
    font-weight: 600;
    cursor: default;
    transition: background 0.15s ease, border-color 0.15s ease;
  }
  .perm-btn:hover {
    background: color-mix(in oklch, var(--color-surface-3, var(--color-surface-2)) 70%, transparent);
  }
  .perm-btn.primary {
    border-color: transparent;
    background: var(--color-violet);
    color: #fff;
  }
  .perm-btn.primary:hover {
    background: color-mix(in oklch, var(--color-violet) 88%, #fff 6%);
  }
</style>

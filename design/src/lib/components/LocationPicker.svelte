<script lang="ts">
  import Icon from './Icon.svelte';
  import { store } from '../store.svelte';
  import { formatSize } from '../format';
  import type { LocationIcon } from '../types';

  let open = $state(false);
  let panel: HTMLElement;

  const freeSpace = $derived(store.currentVolume.capacity - store.currentVolume.used);

  const iconName = (icon: LocationIcon): string => {
    switch (icon) {
      case 'drive': return 'hardDrive';
      case 'externalDrive': return 'externalDrive';
      default: return 'hardDrive';
    }
  };

  function pick(id: string) {
    store.setLocation(id);
    open = false;
  }

  function onDocClick(event: MouseEvent) {
    if (open && panel && !panel.contains(event.target as Node)) open = false;
  }
</script>

<svelte:document onclick={onDocClick} />

<div class="relative" bind:this={panel}>
  <button
    type="button"
    class="btn btn-quiet btn-sm h-[30px]"
    style="color: var(--color-fg)"
    onclick={() => (open = !open)}
    aria-expanded={open}
    aria-haspopup="menu"
    data-od-id="location-button"
  >
    <Icon name={iconName(store.currentLocation.icon)} size={14} />
    <span class="max-w-[130px] truncate font-[550]">{store.currentLocation.name}</span>
    <span class="num text-[11px]" style="color: var(--color-faint)">{formatSize(freeSpace, 0)} 可用</span>
    <Icon name="chevronUp" size={12} class="transition-transform duration-200" style={open ? '' : 'transform: rotate(180deg)'} />
  </button>

  {#if open}
    <div
      class="absolute left-0 top-[calc(100%+6px)] z-30 w-[260px] overflow-hidden rounded-xl border p-1.5 shadow-2xl"
      style="
        background: color-mix(in oklch, var(--color-surface) 96%, transparent);
        border-color: var(--color-border-strong);
        backdrop-filter: blur(18px);
        animation: loc-pop 0.16s cubic-bezier(0.2, 0.9, 0.3, 1.2);
        transform-origin: top left;
      "
      role="menu"
      data-od-id="location-menu"
    >
      <p class="px-2.5 pb-1 pt-1.5 text-[10.5px] font-[600] uppercase tracking-[0.06em]" style="color: var(--color-faint)">
        磁盘
      </p>
      <ul>
        {#each store.diskLocations as loc (loc.id)}
          <li>
            <button
              type="button"
              role="menuitemradio"
              aria-checked={loc.id === store.currentLocId}
              class="loc-item flex w-full items-center gap-2.5 rounded-lg px-2.5 py-[7px] text-left text-[12.5px] transition-colors {loc.id ===
              store.currentLocId
                ? 'loc-item-selected'
                : ''}"
              style={
                loc.id === store.currentLocId
                  ? 'background: color-mix(in oklch, var(--color-accent) 16%, var(--color-surface)); color: var(--color-fg)'
                  : 'color: var(--color-fg)'
              }
              onclick={() => pick(loc.id)}
            >
              <Icon
                name={iconName(loc.icon)}
                size={15}
                class={loc.id === store.currentLocId ? 'text-[var(--color-accent)]' : 'text-[var(--color-muted)]'}
              />
              <span class="min-w-0 flex-1">
                <span class="block truncate font-[550]">{loc.name}</span>
                <span class="num block truncate text-[10.5px]" style="color: var(--color-faint)">
                  {store.diskUsage(loc.id)}
                </span>
              </span>
              {#if loc.id === store.currentLocId}
                <Icon name="check" size={13} class="text-[var(--color-accent)]" stroke={2.2} />
              {/if}
            </button>
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</div>

<style>
  .loc-item:hover {
    background: var(--color-surface-2);
  }
  .loc-item-selected:hover {
    background: color-mix(in oklch, var(--color-accent) 22%, var(--color-surface-2));
  }
  @keyframes loc-pop {
    from {
      opacity: 0;
      transform: scale(0.96) translateY(-4px);
    }
  }
</style>

<script lang="ts">
  import Icon from './Icon.svelte';
  import { store } from '../store.svelte';
  import { formatSize } from '../format';
  import { t } from '../i18n.svelte';

  let open = $state(false);
  let panel: HTMLElement;

  function pickDisk(id: string) {
    store.selectDisk(id);
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
  >
    <Icon name={store.currentVolume?.isRemovable ? 'externalDrive' : 'hardDrive'} size={14} />
    <span class="max-w-[130px] truncate font-[550]">{store.currentVolume?.name ?? t('common.disk')}</span>
    {#if store.currentVolume}
      <span class="num text-[11px]" style="color: var(--color-faint)">
        {formatSize(store.currentVolume.availableBytes, 0)}
      </span>
    {/if}
    <Icon name="chevronUp" size={12} class="transition-transform duration-200" style={open ? '' : 'transform: rotate(180deg)'} />
  </button>

  {#if open}
    <div
      class="absolute left-0 top-[calc(100%+6px)] z-30 w-[272px] overflow-hidden rounded-xl border p-1.5 shadow-2xl"
      style="
        background: color-mix(in oklch, var(--color-surface) 96%, transparent);
        border-color: var(--color-border-strong);
        backdrop-filter: blur(18px);
        animation: loc-pop 0.16s cubic-bezier(0.2, 0.9, 0.3, 1.2);
        transform-origin: top left;
      "
      role="menu"
    >
      <p class="px-2.5 pb-1 pt-1.5 text-[10.5px] font-[600] uppercase tracking-[0.06em]" style="color: var(--color-faint)">
        {t('location.disks')}
      </p>
      <ul>
        {#each store.volumes as vol (vol.id)}
          <li>
            <button
              type="button"
              role="menuitemradio"
              aria-checked={vol.id === store.scopeId}
              class="loc-item flex w-full items-center gap-2.5 rounded-lg px-2.5 py-[7px] text-left text-[12.5px] transition-colors"
              style={vol.id === store.scopeId
                ? 'background: color-mix(in oklch, var(--color-accent) 16%, var(--color-surface)); color: var(--color-fg)'
                : 'color: var(--color-fg)'}
              onclick={() => pickDisk(vol.id)}
            >
              <Icon
                name={vol.isRemovable ? 'externalDrive' : 'hardDrive'}
                size={15}
                class={vol.id === store.scopeId
                  ? 'text-[var(--color-accent)]'
                  : 'text-[var(--color-muted)]'}
              />
              <span class="min-w-0 flex-1">
                <span class="block truncate font-[550]">{vol.name}</span>
                <span class="num block truncate text-[10.5px]" style="color: var(--color-faint)">
                  {t('location.available', [formatSize(vol.availableBytes, 0)])}
                </span>
              </span>
              {#if vol.id === store.scopeId}
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
  @keyframes loc-pop {
    from {
      opacity: 0;
      transform: scale(0.96) translateY(-4px);
    }
  }
</style>

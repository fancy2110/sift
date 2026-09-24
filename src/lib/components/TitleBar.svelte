<script lang="ts">
  import Icon from './Icon.svelte';
  import { store } from '../store.svelte';

  let {
    onScan,
    scanning
  }: { onScan: () => void; scanning: boolean } = $props();
</script>

<header
  class="relative z-30 flex h-[46px] shrink-0 items-center gap-3 px-4"
  style="background: color-mix(in oklch, var(--color-bg) 55%, transparent); backdrop-filter: blur(16px); border-bottom: 1px solid color-mix(in oklch, var(--color-border) 60%, transparent)"
>
  <div class="flex gap-2 pr-1">
    <span class="h-3 w-3 rounded-full" style="background: oklch(0.68 0.17 25)"></span>
    <span class="h-3 w-3 rounded-full" style="background: oklch(0.78 0.14 85)"></span>
    <span class="h-3 w-3 rounded-full" style="background: oklch(0.74 0.17 145)"></span>
  </div>

  <div class="flex items-center gap-2">
    <span
      class="flex h-6 w-6 items-center justify-center rounded-md"
      style="background: var(--color-accent); color: var(--color-accent-contrast); box-shadow: 0 4px 12px -4px var(--color-accent)"
    >
      <Icon name="layers" size={13} stroke={1.9} />
    </span>
    <span class="text-[13px] font-[700] tracking-tight">Sift</span>
  </div>

  <div class="ml-auto flex items-center gap-3">
    <button
      type="button"
      class="switch"
      class:on={store.autoOn}
      onclick={() => store.toggleAuto()}
      role="switch"
      aria-checked={store.autoOn}
      aria-label="自动整理"
      data-od-id="auto-toggle"
    >
      <span class="switch-thumb"></span>
    </button>

    <button
      type="button"
      class="btn btn-primary btn-sm"
      onclick={onScan}
      disabled={scanning}
      data-od-id="scan-button"
    >
      {#if scanning}
        <span style="animation: switch-spin 0.9s linear infinite; display: inline-flex">
          <Icon name="refresh" size={13} />
        </span>
        {store.autoOn ? '正在扫描' : '正在扫描'}
      {:else}
        <Icon name="spark" size={13} />
        {store.autoOn ? '智能扫描' : '磁盘扫描'}
      {/if}
    </button>
  </div>
</header>

<style>
  @keyframes switch-spin {
    to {
      transform: rotate(360deg);
    }
  }

  .switch {
    position: relative;
    width: 38px;
    height: 22px;
    border-radius: 999px;
    flex: none;
    cursor: pointer;
    border: 1px solid var(--color-border);
    background: var(--color-surface-3);
    transition:
      background 0.22s ease,
      border-color 0.22s ease,
      box-shadow 0.22s ease;
  }

  .switch:hover {
    border-color: color-mix(in oklch, var(--color-border) 60%, var(--color-muted));
  }

  .switch.on {
    background: var(--color-accent);
    border-color: transparent;
    box-shadow: 0 2px 8px -2px var(--color-accent);
  }

  .switch-thumb {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 999px;
    background: #fff;
    box-shadow: 0 1px 3px rgb(0 0 0 / 0.3);
    transition: transform 0.22s cubic-bezier(0.34, 1.4, 0.64, 1);
  }

  .switch.on .switch-thumb {
    transform: translateX(16px);
  }

  .switch:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: 2px;
  }
</style>

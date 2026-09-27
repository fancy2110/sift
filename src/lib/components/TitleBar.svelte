<script lang="ts">
  import Icon from './Icon.svelte';
  import { store } from '../store.svelte';
</script>

<header
  data-tauri-drag-region
  class="relative z-30 flex h-[46px] shrink-0 cursor-default items-center gap-3 pl-[84px] select-none"
  style="background: color-mix(in oklch, var(--color-bg) 55%, transparent); backdrop-filter: blur(16px); border-bottom: 1px solid color-mix(in oklch, var(--color-border) 60%, transparent)"
>
  <button class="brand" type="button" onclick={() => store.goHome()}>
    <span
      class="flex h-6 w-6 items-center justify-center rounded-md"
      style="background: var(--color-accent); color: var(--color-accent-contrast); box-shadow: 0 4px 12px -4px var(--color-accent)"
    >
      <Icon name="layers" size={13} stroke={1.9} />
    </span>
    <span class="text-[13px] font-[700] tracking-tight">Sift</span>
  </button>
</header>

<style>
  .brand {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 2px 4px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: inherit;
    cursor: default;
    transition: background 0.15s ease;
  }
  .brand:hover {
    background: color-mix(in oklch, var(--color-surface-2) 70%, transparent);
  }
  .brand:focus-visible {
    outline: 2px solid color-mix(in oklch, var(--color-accent) 60%, transparent);
    outline-offset: 2px;
  }
</style>

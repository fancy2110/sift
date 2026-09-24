<script lang="ts">
  import Icon from './Icon.svelte';
  import { store } from '../store.svelte';
  import { fly } from 'svelte/transition';
</script>

<div class="pointer-events-none fixed bottom-6 right-6 z-50 flex flex-col items-end gap-2">
  {#each store.toasts as toast (toast.id)}
    <div
      class="pointer-events-auto flex items-center gap-2.5 rounded-xl border px-4 py-2.5 text-[13px]"
      style="
        background: color-mix(in oklch, var(--color-surface-2) 94%, transparent);
        border-color: var(--color-border-strong);
        backdrop-filter: blur(12px);
        box-shadow: 0 16px 40px -16px black;
        animation: toast-in 0.22s cubic-bezier(0.2, 0.9, 0.3, 1.1);
      "
      in:fly={{ duration: 200, x: 40 }}
      out:fly={{ duration: 220, x: 60 }}
    >
      <span class="text-[var(--color-ok)]">
        <Icon name="check" size={15} />
      </span>
      {toast.message}
    </div>
  {/each}
</div>

<style>
  @keyframes toast-in {
    from {
      opacity: 0;
    }
  }
</style>

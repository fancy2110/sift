<script lang="ts">
  import Icon from './Icon.svelte';
  import { scanStages } from '../data';

  let {
    progress,
    stage
  }: { progress: number; stage: number } = $props();
</script>

<div
  class="fixed inset-0 z-40 flex items-center justify-center"
  style="background: color-mix(in oklch, var(--color-bg) 72%, transparent); backdrop-filter: blur(6px)"
  role="dialog"
  aria-modal="true"
  aria-label="正在扫描磁盘"
>
  <div class="card w-[380px] p-6" style="background: var(--color-surface)">
    <div class="flex items-center gap-3">
      <div
        class="flex h-10 w-10 items-center justify-center rounded-xl"
        style="background: color-mix(in oklch, var(--color-accent) 18%, var(--color-surface)); color: var(--color-accent)"
      >
        <span style="animation: spin 1.1s linear infinite"><Icon name="refresh" size={20} /></span>
      </div>
      <div>
        <div class="text-[14px] font-[650]">AI 正在分析你的磁盘</div>
        <div class="mt-0.5 text-[12px]" style="color: var(--color-muted)">
          {scanStages[Math.min(stage, scanStages.length - 1)]}
        </div>
      </div>
    </div>

    <div class="mt-5 h-1.5 overflow-hidden rounded-full" style="background: var(--color-surface-3)">
      <div
        class="h-full rounded-full transition-all duration-300"
        style="width: {progress}%; background: var(--color-accent)"
      ></div>
    </div>

    <ul class="mt-4 space-y-1.5">
      {#each scanStages as label, i}
        <li class="flex items-center gap-2 text-[12px]">
          {#if i < stage}
            <span class="text-[var(--color-ok)]"><Icon name="check" size={13} /></span>
            <span style="color: var(--color-muted)">{label}</span>
          {:else if i === stage}
            <span class="text-[var(--color-accent)]"><span style="animation: spin 1s linear infinite; display: inline-flex"><Icon name="refresh" size={13} /></span></span>
            <span class="font-[550]">{label}</span>
          {:else}
            <span style="color: var(--color-faint)"><Icon name="clock" size={13} /></span>
            <span style="color: var(--color-faint)">{label}</span>
          {/if}
        </li>
      {/each}
    </ul>
  </div>
</div>

<style>
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>

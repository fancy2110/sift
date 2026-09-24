<script lang="ts">
  import Treemap from '../lib/components/Treemap.svelte';
  import Icon from '../lib/components/Icon.svelte';
  import LocationPicker from '../lib/components/LocationPicker.svelte';
  import { store } from '../lib/store.svelte';
  import { formatSize } from '../lib/format';

  /** Cross-pane focus: node id focused on either side. */
  let focusNodeId = $state<string | null>(null);
</script>

<div class="relative flex h-full flex-col gap-3 p-3" data-od-id="workspace">
  <!-- top navigation bar: disk picker + persistent breadcrumb -->
  <div class="flex shrink-0 items-center gap-2">
    <div class="glass flex items-center gap-1 rounded-2xl px-2 py-1.5">
      <LocationPicker />

      <div class="mx-1 h-5 w-px" style="background: var(--color-border-strong)"></div>

      <div class="flex items-center gap-0.5 pr-1 text-[12px]">
        {#each store.breadcrumbs as crumb, i (crumb.id)}
          {#if i > 0}
            <Icon name="chevronRight" size={11} class="shrink-0" style="color: var(--color-faint)" />
          {/if}
          <button
            type="button"
            class="crumb h-7 max-w-[160px] truncate rounded-lg px-2"
            class:crumb-current={i === store.breadcrumbs.length - 1}
            onclick={() => store.jumpCrumb(i)}
            data-od-id="crumb-{i}"
          >
            {crumb.name}
          </button>
        {/each}
      </div>
    </div>
  </div>

  <!-- unified borderless canvas: treemap stage + immersive directory list -->
  <div class="flex min-h-0 flex-1 items-stretch justify-center">
    <div class="canvas-body flex w-full max-w-[1180px]">
      <div class="relative min-w-0 flex-1" data-od-id="treemap-stage">
        <Treemap
          entries={store.tileEntries}
          totalSize={store.totalSize}
          pending={!!store.currentNode?.pending}
          {focusNodeId}
          onHoverId={(id) => (focusNodeId = id)}
          onDrill={(id) => store.drillInto(id)}
        />
      </div>

      <aside
        class="immersive-list relative flex w-[268px] shrink-0 flex-col"
        aria-label="文件列表"
        data-od-id="file-list-panel"
      >
        <div class="flex items-center gap-2 px-3.5 pb-1.5 pt-3">
          <h2 class="text-[12px] font-[650]" style="color: var(--color-muted)">文件与文件夹</h2>
          <span class="num ml-auto text-[11px]" style="color: var(--color-faint)"
            >{store.listEntries.length}</span
          >
        </div>

        <ul class="min-h-0 flex-1 overflow-y-auto px-2 pb-2" data-od-id="file-list">
          {#each store.listEntries as entry, i (entry.id)}
            <li
              class="entry-row group flex items-center gap-2 rounded-lg px-2 py-[7px]"
              class:row-focus={focusNodeId === entry.id}
              class:row-dim={!!focusNodeId && focusNodeId !== entry.id}
              style="animation: list-row-in 0.24s cubic-bezier(0.22,1,0.36,1) both; animation-delay: {Math.min(i, 8) * 24}ms"
              onmouseenter={() => (focusNodeId = entry.id)}
              onmouseleave={() => (focusNodeId = null)}
            >
              <span
                class="flex h-[18px] w-[18px] shrink-0 items-center justify-center"
                style="color: var(--color-faint)"
              >
                <Icon name={entry.isDir ? 'folder' : 'hardDrive'} size={14} />
              </span>

              <button
                type="button"
                class="flex min-w-0 flex-1 items-center gap-1.5 rounded-md text-left disabled:cursor-default"
                disabled={!entry.isDir}
                onclick={() => store.drillInto(entry.id)}
              >
                <span class="truncate text-[12.5px] font-[480]">{entry.name}</span>
                {#if entry.isDir}
                  <Icon
                    name="chevronRight"
                    size={12}
                    class="ml-auto shrink-0 transition-transform group-hover:translate-x-0.5"
                    style="color: var(--color-faint)"
                  />
                {/if}
              </button>

              <span class="num shrink-0 text-[11px]" style="color: var(--color-muted)">
                {#if entry.pending}…{:else}{formatSize(entry.size)}{/if}
              </span>
            </li>
          {/each}

          {#if store.listEntries.length === 0}
            <li class="flex h-full flex-col items-center justify-center gap-2 text-[12px]" style="color: var(--color-muted)">
              <Icon name="check" size={20} style="color: var(--color-ok)" />
              此文件夹没有可显示的内容
            </li>
          {/if}
        </ul>
      </aside>
    </div>
  </div>

  <!-- bottom summary: live scan state (findings join here in a later milestone) -->
  <div class="glass flex shrink-0 items-center gap-2.5 self-start rounded-2xl px-4 py-2.5" data-od-id="ai-summary">
    <span
      class="flex h-7 w-7 items-center justify-center rounded-lg"
      style="background: color-mix(in oklch, var(--color-accent) 22%, var(--color-surface)); color: var(--color-accent-hi)"
    >
      <Icon name="layers" size={15} />
    </span>
    {#if store.scanning}
      <span class="text-[12px]">
        正在扫描：<span class="num font-[650]">{store.scannedFiles.toLocaleString()}</span> 个文件 ·
        <span class="num font-[650]">{store.scannedDirs.toLocaleString()}</span> 个文件夹
      </span>
    {:else if store.currentVolume}
      <span class="text-[12px]">
        <span class="num font-[650]">{formatSize(store.currentVolume.availableBytes)}</span>
        可用空间（共 {formatSize(store.currentVolume.totalBytes)}）
      </span>
    {/if}
  </div>
</div>

<style>
  @keyframes list-row-in {
    from {
      opacity: 0;
      transform: translateX(10px);
    }
  }

  .crumb {
    color: var(--color-muted);
    transition: background 0.14s ease, color 0.14s ease;
  }
  .crumb:hover {
    background: color-mix(in oklch, var(--color-surface-2) 80%, transparent);
    color: var(--color-fg);
  }
  .crumb-current {
    color: var(--color-fg);
  }

  .entry-row {
    transition: opacity 0.18s ease;
  }
  .entry-row.row-dim {
    opacity: 0.34;
  }

  .canvas-body {
    border-radius: 20px;
    background: color-mix(in oklch, var(--color-surface) 52%, transparent);
    overflow: hidden;
  }
  .immersive-list {
    background: color-mix(in oklch, var(--color-bg) 30%, transparent);
    box-shadow:
      inset 14px 18px 22px -18px oklch(0% 0 0 / 0.55),
      inset 10px 0 14px -12px oklch(0% 0 0 / 0.45);
  }
</style>

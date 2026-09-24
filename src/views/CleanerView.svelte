<script lang="ts">
  import Treemap from '../lib/components/Treemap.svelte';
  import Icon from '../lib/components/Icon.svelte';
  import LocationPicker from '../lib/components/LocationPicker.svelte';
  import { store } from '../lib/store.svelte';
  import { formatSize } from '../lib/format';
  import type { FileNode } from '../lib/types';
  import { fade, scale } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';

  let focusNodeId = $state<string | null>(null);

  // ---- shared right-click menu ----
  interface Ctx {
    x: number;
    y: number;
    node: FileNode;
    inQueue: boolean;
  }
  let ctx = $state<Ctx | null>(null);
  let workspaceEl: HTMLElement | undefined = $state();

  function openCtx(node: FileNode, cx: number, cy: number) {
    const ws = workspaceEl?.getBoundingClientRect();
    if (!ws) return;
    const W = 200;
    const H = 44;
    let x = cx - ws.left;
    let y = cy - ws.top;
    x = Math.min(Math.max(4, x), ws.width - W - 6);
    y = Math.min(Math.max(4, y), ws.height - H - 6);
    ctx = { x, y, node, inQueue: store.selectedIds.has(node.id) };
  }

  function ctxAction() {
    const c = ctx;
    ctx = null;
    if (!c) return;
    store.toggleSelected(c.node.id);
    store.toast(
      c.inQueue
        ? `已将「${c.node.name}」移出删除队列`
        : `已将「${c.node.name}」加入删除队列`
    );
  }

  // ---- popup geometry ----
  const POPUP_GAP = 8;
  const POPUP_W = 520;
  const POPUP_H = 560;
  let popupGeom = $derived.by(() => {
    const ws = workspaceEl?.getBoundingClientRect();
    const cap = workspaceEl?.querySelector<HTMLElement>('[data-od-id="ai-summary"]')?.getBoundingClientRect();
    if (!ws || !cap) return null;
    const width = Math.min(POPUP_W, ws.width - 24);
    let left = cap.left - ws.left;
    if (left + width > ws.width - 12) left = ws.width - 12 - width;
    const height = Math.min(POPUP_H, cap.top - ws.top - POPUP_GAP);
    const top = cap.top - ws.top - POPUP_GAP - height;
    return { left, top, width, height };
  });

  // Close menus on outside interaction / Esc / navigation.
  $effect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        ctx = null;
        store.drawerOpen = false;
      }
    };
    const onDown = (e: MouseEvent) => {
      const t = e.target as HTMLElement;
      if (ctx && !t.closest('.ctx-menu')) ctx = null;
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('mousedown', onDown, true);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('mousedown', onDown, true);
    };
  });

  $effect(() => {
    void store.currentNodeId;
    ctx = null;
  });

  // Border-growth morph from the capsule rectangle.
  function frameMorph(el: HTMLElement) {
    const end = el.getBoundingClientRect();
    const cap = workspaceEl?.querySelector<HTMLElement>('[data-od-id="ai-summary"]')?.getBoundingClientRect();
    const start = cap ?? end;
    const kx = start.width / end.width;
    const ky = start.height / end.height;
    const dx = start.left - end.left;
    const dy = start.top - end.top;
    return {
      duration: 320,
      easing: cubicOut,
      css: (t: number) => {
        const e = 1 - t;
        return `transform-origin:0 0;transform:translate(${(dx * e).toFixed(2)}px,${(dy * e).toFixed(2)}px) scale(${(kx + (1 - kx) * t).toFixed(4)},${(ky + (1 - ky) * t).toFixed(4)});border-radius:${(16 + 4 * t).toFixed(1)}px;box-shadow:0 ${(8 + 26 * t).toFixed(1)}px ${(20 + 50 * t).toFixed(1)}px -16px oklch(0% 0 0 / ${(0.35 + 0.25 * t).toFixed(3)})`;
      }
    };
  }
</script>

<div bind:this={workspaceEl} class="relative flex h-full flex-col gap-3 p-3" data-od-id="workspace">
  <!-- top navigation -->
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
          >
            {crumb.name}
          </button>
        {/each}
      </div>
    </div>
  </div>

  <!-- unified canvas -->
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
          onContextNode={(node, x, y) => openCtx(node, x, y)}
        />
      </div>

      <aside class="immersive-list flex w-[268px] shrink-0 flex-col" aria-label="文件列表" data-od-id="file-list-panel">
        <div class="flex items-center gap-2 px-3.5 pb-1.5 pt-3">
          <h2 class="text-[12px] font-[650]" style="color: var(--color-muted)">文件与文件夹</h2>
          <span class="num ml-auto text-[11px]" style="color: var(--color-faint)">{store.listEntries.length}</span>
        </div>
        <ul class="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
          {#each store.listEntries as entry, i (entry.id)}
            <li
              class="entry-row group flex items-center gap-2 rounded-lg px-2 py-[7px]"
              class:row-dim={!!focusNodeId && focusNodeId !== entry.id}
              style="animation: list-row-in 0.24s cubic-bezier(0.22,1,0.36,1) both; animation-delay: {Math.min(i, 8) * 24}ms"
              onmouseenter={() => (focusNodeId = entry.id)}
              onmouseleave={() => (focusNodeId = null)}
              oncontextmenu={(e) => {
                e.preventDefault();
                openCtx(entry, e.clientX, e.clientY);
              }}
            >
              <span class="flex h-[18px] w-[18px] shrink-0 items-center justify-center" style="color: var(--color-faint)">
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
                  <Icon name="chevronRight" size={12} class="ml-auto shrink-0 transition-transform group-hover:translate-x-0.5" style="color: var(--color-faint)" />
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

  <!-- backdrop + candidate popup -->
  {#if store.drawerOpen}
    <button
      type="button"
      aria-label="关闭"
      class="popup-backdrop"
      in:fade={{ duration: 160 }}
      out:fade={{ duration: 120 }}
      onclick={() => (store.drawerOpen = false)}
    ></button>
    {#if popupGeom}
      <div
        class="popup-frame absolute z-20 flex flex-col overflow-hidden"
        style="left:{popupGeom.left}px;top:{popupGeom.top}px;width:{popupGeom.width}px;height:{popupGeom.height}px;background:color-mix(in oklch, var(--color-surface) 95%, var(--color-bg));border:1px solid var(--color-border-strong)"
        role="dialog"
        aria-label="清理候选"
        in:frameMorph
        out:frameMorph
        data-od-id="candidate-popup"
      >
        <div class="flex items-center gap-2 px-4 pt-3.5" in:fade={{ duration: 160, delay: 120 }} out:fade={{ duration: 60 }}>
          <h2 class="text-[14px] font-[650]">清理候选</h2>
          <span class="num text-[11px]" style="color: var(--color-faint)">{store.candidateNodes.length} 项</span>
          <button type="button" aria-label="关闭" class="btn-icon ml-auto" onclick={() => (store.drawerOpen = false)}>
            <Icon name="x" size={15} />
          </button>
        </div>
        <ul class="mt-2 min-h-0 flex-1 overflow-y-auto px-3" data-od-id="candidate-list">
          {#each store.candidateNodes as n, i (n.id)}
            <li
              class="flex items-center gap-3 rounded-lg px-2.5 py-2"
              style="animation: cand-row-in 0.24s cubic-bezier(0.22,1,0.36,1) both; animation-delay: {120 + Math.min(i, 8) * 26}ms"
            >
              <button
                type="button"
                role="checkbox"
                aria-checked="true"
                class="check-box flex h-[18px] w-[18px] shrink-0 items-center justify-center rounded-[5px] border"
                onclick={() => store.toggleSelected(n.id)}
                aria-label={n.name}
              >
                <Icon name="check" size={12} stroke={2.4} class="check-icon" />
              </button>
              <span class="min-w-0 flex-1 truncate text-[12.5px] font-[560]">{n.name}</span>
              <span class="num shrink-0 text-[11px]" style="color: var(--color-faint)">{n.path}</span>
              <span class="num w-[64px] shrink-0 text-right text-[12px] font-[600]">{formatSize(n.size)}</span>
            </li>
          {/each}
          {#if store.candidateNodes.length === 0}
            <li class="py-14 text-center text-[12px]" style="color: var(--color-faint)">
              暂未选择项目，可在左侧区块或列表中右键加入
            </li>
          {/if}
        </ul>
        <div class="sheet-hairline flex shrink-0 items-center gap-3 px-4 py-3" in:fade={{ duration: 160, delay: 140 }} out:fade={{ duration: 50 }}>
          <span class="text-[11px]" style="color: var(--color-faint)">移入回收站，可恢复</span>
          <button
            type="button"
            class="btn btn-primary ml-auto min-w-[150px]"
            disabled={store.candidateNodes.length === 0 || store.cleaning}
            onclick={() => store.clean()}
            data-od-id="clean-button"
          >
            {#if store.cleaning}
              <Icon name="refresh" size={14} class="animate-spin" /> 清理中
            {:else}
              <Icon name="trash" size={14} /> 清理 {formatSize(store.selectedBytes)}
            {/if}
          </button>
        </div>
      </div>
    {/if}
  {/if}

  <!-- bottom summary -->
  <button
    type="button"
    class="glass group relative z-30 flex shrink-0 items-center gap-2.5 self-start rounded-2xl px-4 py-2.5 text-left"
    aria-expanded={store.drawerOpen}
    onclick={() => (store.drawerOpen = !store.drawerOpen)}
    data-od-id="ai-summary"
  >
    <span class="flex h-7 w-7 items-center justify-center rounded-lg" style="background: color-mix(in oklch, var(--color-accent) 22%, var(--color-surface)); color: var(--color-accent-hi)">
      <Icon name="layers" size={15} />
    </span>
    {#if store.scanning}
      <span class="text-[12px]">
        正在扫描：<span class="num font-[650]">{store.scannedFiles.toLocaleString()}</span> 文件 ·
        <span class="num font-[650]">{store.scannedDirs.toLocaleString()}</span> 文件夹
      </span>
    {:else}
      <span class="text-[12px]">
        {#if store.candidateNodes.length > 0}
          待清理 <span class="num font-[650]" style="color: var(--color-accent-hi)">{formatSize(store.selectedBytes)}</span>
          <span style="color: var(--color-faint)"> · {store.candidateNodes.length} 项</span>
        {:else}
          <span class="num font-[650]">{formatSize(store.currentVolume?.availableBytes ?? 0)}</span>
          <span style="color: var(--color-faint)"> 可用空间</span>
        {/if}
      </span>
    {/if}
    <Icon name="chevronRight" size={14} class="shrink-0 transition-transform {store.drawerOpen ? 'rotate-90' : 'group-hover:translate-x-0.5'}" style="color: var(--color-faint)" />
  </button>
</div>

<style>
  @keyframes list-row-in {
    from { opacity: 0; transform: translateX(10px); }
  }
  @keyframes cand-row-in {
    from { opacity: 0; transform: translateY(8px); }
  }

  .crumb { color: var(--color-muted); transition: background 0.14s ease, color 0.14s ease; }
  .crumb:hover { background: color-mix(in oklch, var(--color-surface-2) 80%, transparent); color: var(--color-fg); }
  .crumb-current { color: var(--color-fg); }
  .entry-row { transition: opacity 0.18s ease; }
  .entry-row.row-dim { opacity: 0.34; }

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

  .popup-backdrop {
    position: absolute;
    inset: 0;
    z-index: 10;
    border-radius: 20px;
    background: oklch(0 0 0 / 0.34);
    backdrop-filter: blur(2.5px) saturate(0.9);
    -webkit-backdrop-filter: blur(2.5px) saturate(0.9);
  }

  /* soft hairline above the bottom action row */
  .sheet-hairline { position: relative; }
  .sheet-hairline::before {
    content: '';
    position: absolute;
    top: 0;
    left: 14px;
    right: 14px;
    height: 1px;
    background: linear-gradient(90deg, transparent, color-mix(in oklch, var(--color-border-strong) 42%, transparent) 14%, color-mix(in oklch, var(--color-border-strong) 42%, transparent) 86%, transparent);
  }

  .check-box {
    border-color: var(--color-border-strong);
    background: var(--color-accent);
    border-color: var(--color-accent);
    color: var(--color-accent-contrast);
    box-shadow: 0 0 0 3px color-mix(in oklch, var(--color-accent) 22%, transparent);
    transition: transform 0.12s ease;
  }
  .check-box:active { transform: scale(0.88); }
  .check-box:focus-visible { outline: none; box-shadow: 0 0 0 2px var(--color-surface), 0 0 0 4px var(--color-accent); }
  .check-box :global(.check-icon) { transition: transform 0.18s cubic-bezier(0.2, 1.4, 0.4, 1); }

  /* shared right-click menu */
  .ctx-menu {
    position: absolute;
    z-index: 40;
    width: 200px;
    overflow: hidden;
    border-radius: 11px;
    padding: 4px;
    background: color-mix(in oklch, var(--color-surface) 92%, var(--color-bg));
    border: 1px solid var(--color-border-strong);
    box-shadow: 0 2px 8px -2px oklch(0% 0 0 / 0.5), 0 18px 44px -12px oklch(0% 0 0 / 0.6);
    transform-origin: top left;
  }
  .ctx-item {
    display: flex;
    width: 100%;
    align-items: center;
    gap: 10px;
    border-radius: 8px;
    padding: 7px 9px;
    text-align: left;
    font-size: 12.5px;
    color: var(--color-fg);
    transition: background 0.13s ease;
  }
  .ctx-item:hover { background: color-mix(in oklch, var(--color-accent) 16%, var(--color-surface)); }
  .ctx-danger:hover { background: color-mix(in oklch, var(--color-danger) 18%, var(--color-surface)); }
  .ctx-item:focus-visible { outline: none; box-shadow: inset 0 0 0 2px var(--color-accent); }
</style>

{#if ctx}
  <div class="ctx-menu" style="left:{ctx.x}px;top:{ctx.y}px" role="menu" in:scale={{ duration: 120, start: 0.96 }} out:scale={{ duration: 90, start: 0.96, opacity: 0 }}>
    <button type="button" role="menuitem" class="ctx-item {ctx.inQueue ? 'ctx-danger' : ''}" onclick={ctxAction}>
      <Icon name={ctx.inQueue ? 'undo' : 'trash'} size={14} class="shrink-0" />
      {ctx.inQueue ? '从删除队列移除' : '添加到删除队列'}
    </button>
  </div>
{/if}

<script lang="ts">
  import Treemap from '../lib/components/Treemap.svelte';
  import Icon from '../lib/components/Icon.svelte';
  import LocationPicker from '../lib/components/LocationPicker.svelte';
  import { store } from '../lib/store.svelte';
  import { formatSize } from '../lib/format';
  import type { Risk } from '../lib/types';
  import { fly, fade, scale } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';

  // ease-out cubic-bezier(0.22, 1, 0.36, 1), evaluated via Newton iteration
  const morphEase = (() => {
    const p1y = 1;
    const p2y = 1;
    const sample = (t: number) => 3 * (1 - t) * (1 - t) * t * p1y + 3 * (1 - t) * t * t * p2y + t * t * t;
    return (x: number) => sample(Math.min(1, Math.max(0, x)));
  })();

  const riskColor: Record<Risk, string> = {
    safe: 'var(--color-ok)',
    review: 'var(--color-warn)',
    keep: 'var(--color-faint)'
  };

  let detailTab = $state<'candidates' | 'routines'>('candidates');

  /** Cross-pane linkage: name of the directory currently focused on either side. */
  let focusName = $state<string | null>(null);

  /** Right-click menu on an immersive-list row. */
  let listCtx = $state<{
    x: number;
    y: number;
    node: import('../lib/types').FileNode;
    inQueue: boolean;
  } | null>(null);

  function openListContext(
    e: MouseEvent,
    node: import('../lib/types').FileNode
  ) {
    e.preventDefault();
    const host = (e.currentTarget as HTMLElement).closest<HTMLElement>('[data-od-id="file-list-panel"]');
    const r = host?.getBoundingClientRect();
    const MENU_W = 196;
    let x = r ? e.clientX - r.left : 0;
    let y = r ? e.clientY - r.top : 0;
    if (r) {
      x = Math.min(Math.max(0, x), r.width - MENU_W - 4);
      y = Math.min(Math.max(0, y), r.height - 44);
    }
    listCtx = {
      x,
      y,
      node,
      inQueue: !!node.insightId && store.selectedIds.has(node.insightId)
    };
  }

  function confirmListContext() {
    const c = listCtx;
    listCtx = null;
    if (!c) return;
    if (c.inQueue && c.node.insightId) {
      store.toggleSelected(c.node.insightId);
      store.toast(`已将「${c.node.name}」移出删除队列`);
    } else {
      store.addManualCandidate(c.node);
    }
  }

  /** Geometry of the bottom summary capsule — the morph's start/end rectangle. */
  let barRect: DOMRect | null = $state(null);

  /** Workspace root, used to anchor the popup. */
  let workspaceEl: HTMLElement | undefined = $state();

  /** Final geometry of the grown menu, relative to the workspace. */
  let popupGeom = $state<{ left: number; top: number; width: number; height: number } | null>(null);
  const popupStyle = $derived(
    popupGeom
      ? `left:${popupGeom.left}px;top:${popupGeom.top}px;width:${popupGeom.width}px;height:${popupGeom.height}px`
      : ''
  );

  const POPUP_GAP = 8;
  const POPUP_MAX_W = 560;
  const POPUP_MAX_H = 660;

  function layoutPopup(cap: DOMRect) {
    const ws = workspaceEl?.getBoundingClientRect();
    if (!ws) return;
    const width = Math.min(POPUP_MAX_W, ws.width - 24);
    let left = cap.left - ws.left;
    if (left + width > ws.width - 12) left = ws.width - 12 - width;
    const height = Math.max(260, Math.min(POPUP_MAX_H, cap.top - ws.top - POPUP_GAP));
    const top = cap.top - ws.top - POPUP_GAP - height;
    popupGeom = { left, top, width, height };
  }

  function currentCapsuleRect(): DOMRect | undefined {
    return workspaceEl
      ?.querySelector<HTMLElement>('[data-od-id="ai-summary"]')
      ?.getBoundingClientRect();
  }

  function toggleDetail(e: MouseEvent) {
    const cap = (e.currentTarget as HTMLElement).getBoundingClientRect();
    if (!store.drawerOpen) {
      barRect = cap;
      detailTab = 'candidates';
      layoutPopup(cap);
    }
    store.drawerOpen = !store.drawerOpen;
  }

  // Keep the popup anchored if the window is resized while it is open.
  $effect(() => {
    const onResize = () => {
      const cap = currentCapsuleRect();
      if (store.drawerOpen && cap) {
        barRect = cap;
        layoutPopup(cap);
      }
    };
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  });

  // Dismiss the row context menu on outside click / Esc / navigation.
  $effect(() => {
    const close = () => (listCtx = null);
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && close();
    const onDown = (e: MouseEvent) => {
      if (!(e.target as HTMLElement).closest('.list-ctx')) close();
    };
    window.addEventListener('mousedown', onDown, true);
    window.addEventListener('keydown', onKey);
    window.addEventListener('resize', close);
    return () => {
      window.removeEventListener('mousedown', onDown, true);
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('resize', close);
    };
  });

  // Close the menu whenever navigation / scope changes.
  $effect(() => {
    void store.drillPath.length;
    void store.currentLocId;
    void store.treeVersion;
    listCtx = null;
  });

  const summarySafe = $derived(store.visible.filter((i) => i.risk === 'safe').reduce((s, i) => s + i.size, 0));
  const summaryReview = $derived(store.visible.filter((i) => i.risk === 'review').reduce((s, i) => s + i.size, 0));
  const summaryKeep = $derived(store.visible.filter((i) => i.risk === 'keep').reduce((s, i) => s + i.size, 0));
  const hasFindings = $derived(
    store.visible.some((i) => i.risk === 'safe' || i.risk === 'review')
  );
  const detailItems = $derived(store.visible.filter((i) => i.risk !== 'keep'));

  function listIn(
    _el: Element,
    { index, base = 0 }: { index: number; base?: number }
  ) {
    return {
      duration: 280,
      delay: base + Math.min(index, 8) * 28,
      easing: cubicOut,
      css: (t: number) =>
        `opacity:${t};transform:translateX(${(14 * (1 - t)).toFixed(1)}px) scale(${(0.985 + 0.015 * t).toFixed(4)});transform-origin:left center;filter:blur(${(3 * (1 - t)).toFixed(1)}px)`
    };
  }

  /**
   * Border-growth morph: the popup frame grows from the bottom capsule's
   * rectangle into an anchored menu above it, and shrinks back on close.
   * t: 0 = capsule geometry, 1 = popup geometry (enter 0→1, leave 1→0).
   */
  function frameMorph(el: HTMLElement) {
    const end = el.getBoundingClientRect();
    const start = barRect ?? end;
    const duration = 320;
    const kx = start.width / end.width;
    const ky = start.height / end.height;
    const dx = start.left - end.left;
    const dy = start.top - end.top;
    return {
      duration,
      easing: morphEase,
      css: (t: number) => {
        const e = 1 - t;
        return [
          `transform-origin:0 0`,
          `transform:translate(${(dx * e).toFixed(2)}px,${(dy * e).toFixed(2)}px) scale(${(kx + (1 - kx) * t).toFixed(4)},${(ky + (1 - ky) * t).toFixed(4)})`,
          `border-radius:${(16 + 4 * t).toFixed(1)}px`,
          `box-shadow:0 ${(8 + 26 * t).toFixed(1)}px ${(20 + 50 * t).toFixed(1)}px -16px oklch(0% 0 0 / ${(0.35 + 0.25 * t).toFixed(3)})`
        ].join(';');
      }
    };
  }
</script>

<div bind:this={workspaceEl} class="relative flex h-full flex-col gap-3 p-3" data-od-id="workspace">
  <!-- top navigation bar: location + breadcrumb + capacity -->
  <div class="flex shrink-0 items-center gap-2">
    <div class="glass flex items-center gap-1 rounded-2xl px-2 py-1.5">
      <LocationPicker />

      <!-- breadcrumb: persistent; root crumb is the selected disk -->
      <div class="mx-1 h-5 w-px" style="background: var(--color-border-strong)"></div>
      <div class="flex items-center gap-0.5 pr-1 text-[12px]">
        <button
          type="button"
          class="crumb h-7 max-w-[160px] truncate rounded-lg px-2 transition-colors"
          class:crumb-current={store.drillPath.length === 0}
          onclick={() => store.jumpCrumb(-1)}
          data-od-id="crumb-root"
        >
          {store.currentLocation.name}
        </button>
        {#each store.drillPath as seg, i (seg + i)}
          <Icon name="chevronRight" size={11} class="shrink-0" style="color: var(--color-faint)" />
          <button
            type="button"
            class="crumb h-7 max-w-[160px] truncate rounded-lg px-2 transition-colors"
            class:crumb-current={i === store.drillPath.length - 1}
            onclick={() => store.jumpCrumb(i)}
            data-od-id="crumb-{i}"
          >
            {seg}
          </button>
        {/each}
      </div>
    </div>

  </div>

  <!-- unified canvas: morphs between the treemap stage and the full detail list -->
  <div class="flex min-h-0 flex-1 items-stretch justify-center">
    <div class="canvas-body relative w-full max-w-[1180px]">
      <!-- treemap main body + immersive same-canvas directory list -->
      <div class="absolute inset-0 flex items-stretch" data-od-id="mode-map">
          <div class="relative min-w-0 flex-1" data-od-id="treemap-stage">
            {#key store.treeVersion}
              <Treemap
                node={store.currentNode}
                path={store.drillPath}
                navDir={store.navDir}
                selectedIds={store.selectedIds}
                focusInsight={store.focusInsight}
                focusName={focusName}
                onDrill={(name) => store.drillInto(name)}
                onToggleInsight={(id) => store.toggleSelected(id)}
                onAddToDelete={(n) => store.addManualCandidate(n)}
                onHoverName={(n) => (focusName = n)}
              />
            {/key}
          </div>

          <aside
            class="immersive-list relative flex w-[268px] shrink-0 flex-col"
            aria-label="文件列表"
            data-od-id="file-list-panel"
          >
            <div class="flex items-center gap-2 px-3.5 pb-1.5 pt-3">
              <h2 class="text-[12px] font-[650]" style="color: var(--color-muted)">文件与文件夹</h2>
              <span class="num ml-auto text-[11px]" style="color: var(--color-faint)">{store.listEntries.length}</span>
            </div>

            <ul class="min-h-0 flex-1 overflow-y-auto px-2 pb-2" data-od-id="file-list">
              {#each store.listEntries as entry, i (entry.name)}
                {@const insight = entry.insightId}
                <li
                  class="entry-row group flex items-center gap-2 rounded-lg px-2 py-[7px]"
                  class:row-focus={focusName === entry.name}
                  class:row-dim={!!focusName && focusName !== entry.name}
                  in:listIn={{ index: i }}
                  oncontextmenu={(e) => openListContext(e, entry)}
                  onmouseenter={() => (focusName = entry.name)}
                  onmouseleave={() => (focusName = null)}
                >
                  <span
                    class="flex h-[18px] w-[18px] shrink-0 items-center justify-center"
                    style="color: {entry.insightId && entry.risk !== 'keep' && entry.risk ? riskColor[entry.risk] : 'var(--color-faint)'}"
                  >
                    {#if entry.insightId && entry.risk === 'keep'}
                      <Icon name="shield" size={13} />
                    {:else}
                      <Icon name={entry.children ? 'folder' : 'hardDrive'} size={14} />
                    {/if}
                  </span>

                  <button
                    type="button"
                    class="flex min-w-0 flex-1 items-center gap-1.5 rounded-md text-left disabled:cursor-default"
                    disabled={!entry.children?.length}
                    onclick={() => entry.children && store.drillInto(entry.name)}
                    onmouseenter={() => insight && (store.focusInsight = insight)}
                    onmouseleave={() => (store.focusInsight = null)}
                  >
                    <span class="truncate text-[12.5px] {insight ? 'font-[600]' : 'font-[480]'}">{entry.name}</span>
                    {#if entry.children}
                      <Icon
                        name="chevronRight"
                        size={12}
                        class="ml-auto shrink-0 transition-transform group-hover:translate-x-0.5"
                        style="color: var(--color-faint)"
                      />
                    {/if}
                  </button>

                  <span class="num shrink-0 text-[11px]" style="color: var(--color-muted)">
                    {formatSize(entry.size)}
                  </span>
                </li>
              {/each}

              {#if store.listEntries.length === 0}
                <li class="flex h-full flex-col items-center justify-center gap-2 text-[12px]" style="color: var(--color-muted)">
                  <Icon name="check" size={20} style="color: var(--color-ok)" />
                  此文件夹没有可整理的内容
                </li>
              {/if}
            </ul>

            {#if listCtx}
              <!-- Row right-click: add to / remove from the deletion queue. -->
              <div
                class="list-ctx absolute z-40 overflow-hidden rounded-xl py-1"
                style="left: {listCtx.x}px; top: {listCtx.y}px; width: 196px"
                role="menu"
                in:scale={{ duration: 130, start: 0.96 }}
                out:scale={{ duration: 110, start: 0.96, opacity: 0 }}
              >
                {#if listCtx.node.risk === 'keep'}
                  <button
                    type="button"
                    role="menuitem"
                    class="list-ctx-item list-ctx-off flex w-full items-center gap-2.5 px-3 py-2 text-left text-[12.5px]"
                    disabled
                  >
                    <Icon name="shield" size={14} class="shrink-0" />
                    <span class="min-w-0 flex-1">AI 已保护，不可删除</span>
                  </button>
                {:else}
                  <button
                    type="button"
                    role="menuitem"
                    class="list-ctx-item flex w-full items-center gap-2.5 px-3 py-2 text-left text-[12.5px]"
                    class:list-ctx-off={listCtx.node.deletable === false}
                    class:list-ctx-danger={listCtx.inQueue}
                    disabled={listCtx.node.deletable === false}
                    onclick={confirmListContext}
                    data-od-id="list-ctx-action"
                  >
                    <Icon name={listCtx.inQueue ? 'undo' : 'trash'} size={14} class="shrink-0" />
                    <span class="min-w-0 flex-1">
                      {listCtx.inQueue ? '从删除队列移除' : '添加到删除队列'}
                    </span>
                    {#if listCtx.node.deletable === false}
                      <span class="shrink-0 text-[10.5px]" style="color: var(--color-faint)">无权限</span>
                    {/if}
                  </button>
                {/if}
              </div>
            {/if}
          </aside>
        </div>
    </div>
  </div>
  {#if store.drawerOpen}
    <!-- dimming backdrop, behind both popup and the capsule -->
    <button
      type="button"
      aria-label="关闭"
      class="popup-backdrop"
      in:fade={{ duration: 180 }}
      out:fade={{ duration: 140 }}
      onclick={() => (store.drawerOpen = false)}
    ></button>

    <!-- popup: the capsule's border grows into this anchored menu -->
    <div
      class="popup-frame absolute z-20 overflow-hidden"
      style="{popupStyle};border-radius:20px;background:color-mix(in oklch, var(--color-surface) 94%, var(--color-bg));border:1px solid var(--color-border-strong)"
      in:frameMorph
      out:frameMorph
      role="dialog"
      aria-label="清理候选"
      data-od-id="mode-detail"
    >
      <div
        class="flex h-full flex-col"
        in:fade={{ duration: 200, delay: 140 }}
        out:fade={{ duration: 80 }}
      >
        <div class="flex items-center gap-1 px-3 pt-2.5">
          <div
            class="segmented relative flex flex-1 rounded-[9px] p-[2px]"
            role="tablist"
            aria-label="详情面板"
          >
            <span
              class="segmented-thumb absolute inset-y-[2px] left-[2px] w-[calc(50%-2px)] rounded-[7px]"
              style="transform: translateX({detailTab === 'routines' ? '100%' : '0%'});"
              aria-hidden="true"
            ></span>
            <button
              type="button"
              role="tab"
              aria-selected={detailTab === 'candidates'}
              class="detail-tab relative z-10 flex flex-1 items-center justify-center gap-1.5 rounded-[7px] py-[5px] text-[12px]"
              class:detail-tab-on={detailTab === 'candidates'}
              onclick={() => (detailTab = 'candidates')}
              data-od-id="tab-candidates"
            >
              <Icon name="spark" size={12.5} />
              清理候选
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={detailTab === 'routines'}
              class="detail-tab relative z-10 flex flex-1 items-center justify-center gap-1.5 rounded-[7px] py-[5px] text-[12px]"
              class:detail-tab-on={detailTab === 'routines'}
              onclick={() => (detailTab = 'routines')}
              data-od-id="tab-routines"
            >
              <Icon name="clock" size={12.5} />
              例行任务
            </button>
          </div>
          <button
            type="button"
            aria-label="关闭"
            class="btn btn-quiet btn-sm shrink-0"
            onclick={() => (store.drawerOpen = false)}
            data-od-id="close-detail"
          >
            <Icon name="x" size={14} />
          </button>
        </div>

        <div class="min-h-0 flex-1 overflow-hidden">
          <div
            class="pager-track flex h-full w-[200%]"
            style="transform: translateX({detailTab === 'routines' ? '-50%' : '0%'}); transition: transform 0.3s cubic-bezier(0.22, 1, 0.36, 1)"
          >
          <div class="flex h-full w-1/2 min-h-0 shrink-0 flex-col">
            <ul class="finding-list mt-2 min-h-0 flex-1 overflow-y-auto px-2" data-od-id="finding-list">
              {#each detailItems as item, i (item.id)}
                {@const checked = store.isSelected(item.id)}
                <li
                  class="finding-row flex items-start gap-3 rounded-lg px-2.5 py-2.5"
                  class:finding-off={!checked}
                  in:listIn={{ index: i, base: 160 }}
                  onmouseenter={() => (store.focusInsight = item.id)}
                  onmouseleave={() => (store.focusInsight = null)}
                >
                  <button
                    type="button"
                    role="checkbox"
                    aria-checked={checked}
                    onclick={() => store.toggleSelected(item.id)}
                    class="check-box mt-0.5 flex h-[18px] w-[18px] shrink-0 items-center justify-center rounded-[5px] border"
                    class:check-on={checked}
                  >
                    <Icon name="check" size={12} stroke={2.4} class="check-icon" />
                  </button>

                  <div class="min-w-0 flex-1">
                    <div class="flex items-center gap-2">
                      <span class="h-1.5 w-1.5 shrink-0 rounded-full" style="background: {riskColor[item.risk]}"></span>
                      <span class="truncate text-[13px] font-[600]">{item.title}</span>
                      {#if item.learned}
                        <span
                          class="shrink-0 rounded px-1.5 py-[1px] text-[10px] font-[600]"
                          style="background: color-mix(in oklch, var(--color-accent) 18%, var(--color-surface)); color: var(--color-accent-hi)"
                        >
                          来自习惯
                        </span>
                      {/if}
                    </div>
                    <p class="mt-0.5 text-[11.5px] leading-snug" style="color: var(--color-muted)">{item.reason}</p>
                    <p class="num mt-0.5 truncate text-[10.5px]" style="color: var(--color-faint)">{item.path}</p>
                  </div>

                  <span class="finding-size num shrink-0 text-[13px] font-[650]" style="transition: color 0.2s ease">{formatSize(item.size)}</span>
                </li>
              {/each}

              {#if detailItems.length === 0}
                <li class="flex flex-col items-center gap-2 py-14 text-center text-[12.5px]" style="color: var(--color-muted)">
                  <Icon name="check" size={22} style="color: var(--color-ok)" />
                  没有待处理内容{#if store.cleanedBytes > 0}，已释放 {formatSize(store.cleanedBytes)}{/if}
                </li>
              {/if}
            </ul>

            <!-- single primary action -->
            <div class="sheet-hairline-t flex shrink-0 items-center gap-3 px-4 py-3">
              <span class="text-[11.5px]" style="color: var(--color-faint)">移入废纸篓，可恢复</span>
              <button
                type="button"
                class="btn btn-primary ml-auto min-w-[150px]"
                disabled={store.candidates.length === 0 || store.cleaning}
                onclick={async () => {
                  await store.clean(false);
                  store.drawerOpen = false;
                }}
                data-od-id="clean-button"
              >
                {#if store.cleaning}
                  <span class="spin" style="display: inline-flex"><Icon name="refresh" size={14} /></span>
                  清理中
                {:else}
                  <Icon name="trash" size={14} />
                  清理 {formatSize(store.selectedBytes)}
                {/if}
              </button>
            </div>
          </div>
          <!-- routines tab -->
          <div
            class="flex h-full w-1/2 min-h-0 shrink-0 flex-col overflow-y-auto px-4 py-4"
          >
            <p class="text-[11.5px] leading-relaxed" style="color: var(--color-faint)">
              AI 记录你每次扫描后的选择，重复的整理决策会沉淀为例行任务；自动整理{#if store.autoOn}已开启{:else}关闭中、仅提醒{/if}。
            </p>
            <ul class="mt-3 space-y-1.5" data-od-id="routines-list">
              {#each store.routines as r, i (r.id)}
                <li
                  class="routine-card flex items-center gap-3 rounded-xl px-3 py-2.5"
                  style="background: color-mix(in oklch, var(--color-surface-2) 70%, transparent); border: 1px solid var(--color-border)"
                  in:listIn={{ index: i, base: 80 }}
                  out:fly={{ x: -24, duration: 180 }}
                >
                  <span
                    class="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg"
                    style="background: color-mix(in oklch, var(--color-accent) {r.autoMode === 'auto' ? '22%' : '10%'}, var(--color-surface)); color: {r.autoMode === 'auto' ? 'var(--color-accent-hi)' : 'var(--color-muted)'}; opacity: {store.runningRoutineId === r.id ? 0.6 : 1}"
                  >
                    {#if store.runningRoutineId === r.id}
                      <Icon name="refresh" size={14} class="animate-spin" />
                    {:else}
                      <Icon name={r.autoMode === 'auto' ? 'bolt' : 'clock'} size={14} />
                    {/if}
                  </span>
                  <div class="min-w-0">
                    <p class="text-[13px] font-[600]">{r.title}</p>
                    <p class="mt-0.5 text-[11px]" style="color: var(--color-faint)">{r.cadence}</p>
                  </div>
                  <span
                    class="num ml-auto shrink-0 rounded-md px-2 py-1 text-[11px] font-[600]"
                    style="background: color-mix(in oklch, var(--color-accent) 14%, var(--color-surface)); color: var(--color-accent-hi)"
                  >
                    ≈{formatSize(r.avgSize)}
                  </span>
                  <span class="hidden w-16 shrink-0 text-right text-[11px] md:block" style="color: var(--color-muted)">
                    {r.autoMode === 'auto' ? '自动执行' : '执行前确认'}
                  </span>
                  <div class="flex shrink-0 items-center gap-1">
                    <button
                      type="button"
                      class="btn-icon routine-run h-7 w-7 rounded-lg"
                      title="立即启动"
                      aria-label="启动{r.title}"
                      disabled={store.runningRoutineId !== null}
                      onclick={() => store.runRoutine(r.id)}
                      data-od-id="routine-run-{r.id}"
                    >
                      <Icon name="bolt" size={13} />
                    </button>
                    <button
                      type="button"
                      class="btn-icon routine-del h-7 w-7 rounded-lg"
                      title="删除"
                      aria-label="删除{r.title}"
                      onclick={() => store.deleteRoutine(r.id)}
                      data-od-id="routine-del-{r.id}"
                    >
                      <Icon name="trash" size={13} />
                    </button>
                  </div>
                </li>
              {/each}
            </ul>
            {#if store.routines.length === 0}
              <div
                class="mt-6 flex flex-col items-center gap-2 text-center"
                in:fade={{ duration: 180 }}
                data-od-id="routines-empty"
              >
                <Icon name="clock" size={22} style="color: var(--color-faint)" />
                <p class="text-[12px]" style="color: var(--color-muted)">暂无疑似例行任务</p>
                <p class="text-[11px]" style="color: var(--color-faint)">重复的整理决策会在这里自动沉淀</p>
              </div>
            {/if}
          </div>
          </div>
        </div>
      </div>
    </div>
  {/if}


  <!-- bottom: AI summary bar — its rectangle is the morph origin -->
  <button
    type="button"
    class="glass group relative z-30 flex shrink-0 items-center gap-2.5 self-start rounded-2xl px-4 py-2.5 text-left"
    aria-expanded={store.drawerOpen}
    onclick={toggleDetail}
    data-od-id="ai-summary"
  >
    <span
      class="flex h-7 w-7 items-center justify-center rounded-lg"
      style="background: color-mix(in oklch, var(--color-accent) 22%, var(--color-surface)); color: var(--color-accent-hi)"
    >
      <Icon name="spark" size={15} />
    </span>
    {#if hasFindings}
      <span class="text-[12px] leading-snug">
        可安全释放 <span class="num font-[650]" style="color: var(--color-ok)">{formatSize(summarySafe)}</span>
        <span style="color: var(--color-muted)">
          · <span class="num" style="color: var(--color-warn)">{formatSize(summaryReview)}</span> 建议确认
          · <span class="num">{formatSize(summaryKeep)}</span> 已保护
        </span>
      </span>
    {:else}
      <span class="text-[12px]" style="color: var(--color-muted)">
        已释放 <span class="num font-[650]" style="color: var(--color-ok)">{formatSize(store.cleanedBytes)}</span>
      </span>
    {/if}
    {#if store.candidates.length > 0}
      <span
        class="num ml-1 flex shrink-0 items-center gap-1 rounded-lg px-1.5 py-0.5 text-[11.5px] font-[650]"
        style="background: color-mix(in oklch, var(--color-accent) 18%, var(--color-surface)); color: var(--color-accent-hi)"
      >
        <Icon name="trash" size={12} />
        {store.candidates.length} 项 · {formatSize(store.selectedBytes)}
      </span>
    {/if}
    <Icon
      name="chevronRight"
      size={14}
      class="shrink-0 transition-transform duration-200 {store.drawerOpen ? 'rotate-90' : 'group-hover:translate-x-0.5'}"
      style="color: var(--color-faint)"
    />
  </button>
</div>

<style>
  .crumb {
    color: var(--color-muted);
  }
  .crumb:hover {
    background: color-mix(in oklch, var(--color-surface-2) 80%, transparent);
    color: var(--color-fg);
  }
  .crumb-current {
    color: var(--color-fg);
  }

  .entry-row {
    transition: color 0.14s ease, background 0.16s ease, opacity 0.18s ease;
  }
  /* cross-pane focus: the linked row lifts without looking "selected" */
  .entry-row.row-focus {
    background: color-mix(in oklch, var(--color-surface-2) 78%, transparent);
    box-shadow:
      0 1px 2px oklch(0% 0 0 / 0.22),
      0 6px 16px -10px oklch(0% 0 0 / 0.5);
  }
  .entry-row.row-dim {
    opacity: 0.34;
  }

  .segmented {
    background: color-mix(in oklch, var(--color-bg) 55%, transparent);
    box-shadow: inset 0 0 0 1px var(--color-border);
  }
  .segmented-thumb {
    background: color-mix(in oklch, var(--color-accent) 22%, var(--color-surface-2));
    box-shadow:
      inset 0 0 0 1px color-mix(in oklch, var(--color-accent) 45%, transparent),
      0 1px 4px -1px oklch(0% 0 0 / 0.5);
    transition: transform 0.24s cubic-bezier(0.4, 0, 0.2, 1);
  }

  .detail-tab {
    color: var(--color-muted);
    transition: color 0.18s ease;
  }
  .detail-tab:hover {
    color: var(--color-fg);
  }
  .detail-tab:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--color-surface), 0 0 0 4px var(--color-accent);
  }
  .detail-tab-on,
  .detail-tab-on:hover {
    color: var(--color-accent-hi);
  }

  /* dimming backdrop behind the grown popup */
  .popup-backdrop {
    position: absolute;
    inset: 0;
    z-index: 10;
    border-radius: 20px;
    background: oklch(0 0 0 / 0.34);
    backdrop-filter: blur(2.5px) saturate(0.9);
    -webkit-backdrop-filter: blur(2.5px) saturate(0.9);
  }

  /* unified canvas: morph host for map & detail modes */
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

  .check-box {
    border-color: var(--color-border-strong);
    transition:
      background 0.16s ease,
      border-color 0.16s ease,
      box-shadow 0.2s ease,
      transform 0.12s ease;
  }
  .check-box:hover {
    border-color: var(--color-accent);
  }
  .check-box:active {
    transform: scale(0.88);
  }
  .check-box:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--color-surface), 0 0 0 4px var(--color-accent);
  }

  /* immersive-list row right-click menu */
  .list-ctx {
    background: color-mix(in oklch, var(--color-surface) 88%, var(--color-bg));
    border: 1px solid var(--color-border-strong);
    box-shadow:
      0 2px 8px -2px oklch(0% 0 0 / 0.5),
      0 18px 44px -12px oklch(0% 0 0 / 0.6);
    transform-origin: top left;
  }
  .list-ctx-item {
    color: var(--color-fg);
    transition: background 0.13s ease, color 0.13s ease;
  }
  .list-ctx-item:hover:not(:disabled) {
    background: color-mix(in oklch, var(--color-accent) 16%, var(--color-surface));
  }
  .list-ctx-danger:hover:not(:disabled) {
    background: color-mix(in oklch, var(--color-danger) 18%, var(--color-surface));
  }
  .list-ctx-item:focus-visible {
    outline: none;
    box-shadow: inset 0 0 0 2px var(--color-accent);
  }
  .list-ctx-off {
    color: var(--color-muted);
    cursor: not-allowed;
  }
  .check-box :global(.check-icon) {
    transition: transform 0.18s cubic-bezier(0.2, 1.4, 0.4, 1);
    transform: scale(0.6);
  }
  .check-box.check-on {
    background: var(--color-accent);
    border-color: var(--color-accent);
    color: var(--color-accent-contrast);
    box-shadow: 0 0 0 3px color-mix(in oklch, var(--color-accent) 22%, transparent);
  }
  .check-box.check-on :global(.check-icon) {
    transform: scale(1);
  }

  .spin {
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  /* soft hairline rule: fades out toward both edges instead of a hard line */
  .sheet-hairline-t {
    position: relative;
  }
  .sheet-hairline-t::before {
    content: '';
    position: absolute;
    top: 0;
    left: 14px;
    right: 14px;
    height: 1px;
    background: linear-gradient(
      90deg,
      transparent,
      color-mix(in oklch, var(--color-border-strong) 42%, transparent) 14%,
      color-mix(in oklch, var(--color-border-strong) 42%, transparent) 86%,
      transparent
    );
  }

  /* gentle separators between finding rows */
  .finding-list {
    margin-bottom: 2px;
  }
  .finding-list li + li {
    position: relative;
  }
  .finding-list li + li::before {
    content: '';
    position: absolute;
    top: 0;
    left: 12px;
    right: 12px;
    height: 1px;
    background: color-mix(in oklch, var(--color-border-strong) 34%, transparent);
  }

  /* candidate row: hover lift + smooth checked/unchecked dimming */
  .finding-row {
    cursor: default;
    transition:
      background 0.18s ease,
      box-shadow 0.2s ease,
      transform 0.18s cubic-bezier(0.22, 1, 0.36, 1),
      opacity 0.2s ease;
    will-change: transform;
  }
  .finding-row:hover {
    background: color-mix(in oklch, var(--color-surface-2) 72%, transparent);
    transform: translateY(-1px);
    box-shadow:
      0 1px 2px oklch(0% 0 0 / 0.22),
      0 6px 16px -8px oklch(0% 0 0 / 0.45);
  }
  .finding-row:active {
    transform: translateY(0) scale(0.995);
  }
  /* unchecked candidates recede quietly without disappearing */
  .finding-row.finding-off {
    opacity: 0.55;
  }
  .finding-row.finding-off:hover {
    opacity: 0.9;
  }
  .finding-row.finding-off :global(.finding-size) {
    color: var(--color-muted);
  }
</style>

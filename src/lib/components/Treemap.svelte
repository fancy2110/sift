<script lang="ts">
  import { cubicIn, cubicOut, backOut } from 'svelte/easing';
  import { fade, scale } from 'svelte/transition';
  import { formatSize } from '../format';
  import type { Node, Risk } from '../types';
  import Icon from './Icon.svelte';

  let {
    entries,
    totalSize,
    selectedIds = new Set<string>(),
    focusFinding = null,
    focusId = null,
    onDrill,
    onToggleFinding,
    onAddToDelete,
    onHoverId
  }: {
    entries: Node[];
    totalSize: number;
    selectedIds?: Set<string>;
    focusFinding?: string | null;
    focusId?: string | null;
    onDrill?: (id: string) => void;
    onToggleFinding?: (findingId: string) => void;
    onAddToDelete?: (node: Node) => void;
    onHoverId?: (id: string | null) => void;
  } = $props();

  const OTHER_KEY = '__other__';

  const CATS = [
    'var(--color-cat-1)',
    'var(--color-cat-2)',
    'var(--color-cat-3)',
    'var(--color-cat-4)',
    'var(--color-cat-5)',
    'var(--color-cat-6)',
    'var(--color-cat-7)',
    'var(--color-cat-8)'
  ];

  interface Box {
    x: number;
    y: number;
    w: number;
    h: number;
  }
  interface Tile extends Box {
    key: string;
    node: Node;
    color: string;
    fill: string;
    findingId?: string;
    risk?: Risk;
    hasChildren: boolean;
    other?: boolean;
    count?: number;
  }

  // ---------- measured stage size ----------
  let stage: HTMLDivElement;
  let W = $state(800);
  let H = $state(600);

  $effect(() => {
    if (!stage) return;
    const ro = new ResizeObserver((list) => {
      const r = list[0].contentRect;
      W = Math.max(120, r.width);
      H = Math.max(120, r.height);
    });
    ro.observe(stage);
    return () => ro.disconnect();
  });

  // ---------- squarified treemap ----------
  interface V {
    v: number;
    n: Node;
  }

  function worst(vals: number[], cw: number, ch: number, total: number): number {
    const scaleArea = (cw * ch) / total;
    const horizontal = cw >= ch;
    const fixed = horizontal ? cw : ch;
    const sum = vals.reduce((a, b) => a + b, 0);
    const t = (sum * scaleArea) / fixed;
    let worstR = 0;
    for (const v of vals) {
      const d = (v * scaleArea) / t;
      const r = Math.max(d / t, t / d);
      if (r > worstR) worstR = r;
    }
    return worstR;
  }

  function squarify(items: V[], area: Box, total: number): { box: Box; node: Node }[] {
    const out: { box: Box; node: Node }[] = [];
    let { x: cx, y: cy, w: cw, h: ch } = area;
    let row: V[] = [];
    let i = 0;
    let rem = total;

    const flush = () => {
      const sum = row.reduce((a, b) => a + b.v, 0);
      const scaleArea = (cw * ch) / rem;
      const horizontal = cw >= ch;
      const fixed = horizontal ? cw : ch;
      const t = (sum * scaleArea) / fixed;
      if (horizontal) {
        let cur = cx;
        for (const it of row) {
          const d = (it.v * scaleArea) / t;
          out.push({ box: { x: cur, y: cy, w: d, h: t }, node: it.n });
          cur += d;
        }
        cy += t;
        ch -= t;
      } else {
        let cur = cy;
        for (const it of row) {
          const d = (it.v * scaleArea) / t;
          out.push({ box: { x: cx, y: cur, w: t, h: d }, node: it.n });
          cur += d;
        }
        cx += t;
        cw -= t;
      }
      rem -= sum;
      row = [];
    };

    while (i < items.length) {
      const item = items[i];
      if (row.length > 0) {
        const before = worst(row.map((r) => r.v), cw, ch, rem);
        const after = worst([...row.map((r) => r.v), item.v], cw, ch, rem);
        if (after < before) {
          row.push(item);
          i++;
        } else {
          flush();
        }
      } else {
        row.push(item);
        i++;
      }
    }
    if (row) flush();
    return out;
  }

  const PAD = 14;
  const GAP = 0;
  const MIN_W = 88;
  const MIN_H = 48;

  let hoverKey = $state<string | null>(null);
  let mouse = $state({ x: 0, y: 0 });
  let otherOpen = $state(false);
  let query = $state('');

  // ---------- tile context menu ----------
  interface CtxState {
    tile: Tile;
    x: number;
    y: number;
  }
  let ctx = $state<CtxState | null>(null);
  const MENU_W = 224;
  const MENU_H = 64;

  function openContextMenu(t: Tile, e: MouseEvent) {
    e.preventDefault();
    if (t.other) return;
    const r = stage.getBoundingClientRect();
    let x = e.clientX - r.left;
    let y = e.clientY - r.top;
    x = Math.min(x, W - MENU_W - 4);
    y = Math.min(y, H - MENU_H - 4);
    ctx = { tile: t, x: Math.max(4, x), y: Math.max(4, y) };
    hoverKey = t.key;
  }

  function closeContextMenu() {
    ctx = null;
  }

  function confirmContextAdd() {
    if (!ctx) return;
    onAddToDelete?.(ctx.tile.node);
    closeContextMenu();
  }

  $effect(() => {
    if (!ctx || otherOpen) return;
    const onDown = (e: MouseEvent) => {
      const r = stage.getBoundingClientRect();
      const x = e.clientX - r.left;
      const y = e.clientY - r.top;
      if (x < ctx!.x || x > ctx!.x + MENU_W || y < ctx!.y || y > ctx!.y + MENU_H) closeContextMenu();
    };
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && closeContextMenu();
    window.addEventListener('mousedown', onDown, true);
    window.addEventListener('keydown', onKey);
    window.addEventListener('resize', closeContextMenu);
    return () => {
      window.removeEventListener('mousedown', onDown, true);
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('resize', closeContextMenu);
    };
  });

  const layout = $derived.by<{ tiles: Tile[]; folded: Node[] }>(() => {
    void W;
    void H;
    const kids = [...entries]
      .filter((c) => c.size > 0 || c.pending)
      .sort((a, b) => b.size - a.size);
    const total = totalSize || kids.reduce((s, k) => s + k.size, 0) || 1;

    const full: Box = {
      x: PAD,
      y: PAD,
      w: Math.max(80, W - PAD * 2),
      h: Math.max(80, H - PAD * 2)
    };

    const buildTile = (raw: Box, child: Node, i: number, other = false, count?: number): Tile => ({
      x: raw.x + GAP / 2,
      y: raw.y + GAP / 2,
      w: raw.w - GAP,
      h: raw.h - GAP,
      key: other ? OTHER_KEY : child.id,
      node: child,
      color: CATS[i % CATS.length],
      fill: other ? 'url(#tm-g-other)' : `url(#tm-g-${i % CATS.length})`,
      findingId: child.insightId,
      risk: child.risk,
      hasChildren: child.isDir,
      other,
      count
    });

    const stageArea = full.w * full.h;

    const chooseGamma = (sizes: number[]): number => {
      if (sizes.length <= 1) return 1;
      const target = MIN_W * MIN_H * 1.45;
      const feasible = (g: number): boolean => {
        let minW = Infinity;
        let sum = 0;
        for (const s of sizes) {
          const w = Math.pow(s, g);
          sum += w;
          if (w < minW) minW = w;
        }
        return (minW / sum) * stageArea >= target;
      };
      if (feasible(1)) return 1;
      let lo = 0.35;
      const hi = 1;
      if (!feasible(lo)) return lo;
      for (let k = 0; k < 22; k++) {
        const mid = (lo + hi) / 2;
        if (feasible(mid)) lo = mid;
        else break;
      }
      return lo;
    };

    let nReal = kids.length;
    for (let guard = 0; guard <= kids.length + 1; guard++) {
      const realKids = kids.slice(0, nReal);
      const folded = kids.slice(nReal);
      const foldedSum = folded.reduce((s, k) => s + k.size, 0);

      const gamma = chooseGamma(realKids.map((k) => k.size));
      const values: V[] = realKids.map((n) => ({ v: Math.max(1, n.size) ** gamma, n }));
      if (folded.length) {
        const foldedWeight = folded.reduce((s, k) => s + Math.max(1, k.size) ** gamma, 0);
        values.push({ v: foldedWeight, n: { ...folded[0], name: `其他 ${folded.length} 项`, size: foldedSum } });
      }
      const weightTotal = values.reduce((s, v) => s + v.v, 0);

      const laid = squarify(values, full, weightTotal);
      const tiles: Tile[] = [];
      let underSized = false;

      laid.forEach(({ box: raw, node: child }, i) => {
        const isOther = !!folded.length && i === laid.length - 1;
        if (raw.w - GAP < MIN_W || raw.h - GAP < MIN_H) {
          underSized = true;
          return;
        }
        if (isOther) tiles.push(buildTile(raw, child, i, true, folded.length));
        else tiles.push(buildTile(raw, child, i));
      });

      if (underSized) {
        if (nReal === 0) {
          const synth: Node = synthNode(`其他 ${kids.length} 项`, total);
          return { tiles: [buildTile(full, synth, 0, true, kids.length)], folded: kids };
        }
        nReal -= 1;
        continue;
      }

      folded.sort((a, b) => b.size - a.size);
      return { tiles, folded };
    }

    return { tiles: [], folded: kids };
  });

  const tiles = $derived(layout.tiles);
  const foldedKids = $derived(layout.folded);

  const otherRows = $derived(
    query.trim()
      ? foldedKids.filter((k) => k.name.toLowerCase().includes(query.trim().toLowerCase()))
      : foldedKids
  );
  const otherMaxSize = $derived(foldedKids[0]?.size ?? 1);
  const otherBytes = $derived(foldedKids.reduce((s, k) => s + k.size, 0));

  const tileByKey = $derived(new Map(tiles.map((t) => [t.key, t])));
  const hovered = $derived(hoverKey ? (tileByKey.get(hoverKey) ?? null) : null);

  function isChosen(t: Tile): boolean {
    return !!t.findingId && selectedIds.has(t.findingId);
  }

  function isFocusTile(t: Tile): boolean {
    if (focusId) return focusId === OTHER_KEY ? !!t.other : t.key === focusId;
    return hoverKey === t.key;
  }

  function rectStyle(t: Tile): string {
    const isHover = isFocusTile(t);
    return [
      `cursor: ${t.other || t.findingId || t.hasChildren ? 'pointer' : 'default'}`,
      `fill: ${t.fill}`,
      `stroke: ${isHover ? 'oklch(0.985 0.005 250)' : 'oklch(0.13 0.01 255 / 0.55)'}`,
      `stroke-width: ${isHover ? 2 : 1}px`,
      `filter: brightness(${isHover ? 1.12 : 1})`,
      'transition: stroke 0.16s ease, filter 0.16s ease, opacity 0.18s ease'
    ].join(';');
  }

  function click(t: Tile) {
    if (t.other) {
      otherOpen = true;
      query = '';
      return;
    }
    if (t.findingId) {
      onToggleFinding?.(t.findingId);
      return;
    }
    if (t.hasChildren) onDrill?.(t.node.id);
  }

  function onKeyDown(t: Tile, event: KeyboardEvent) {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      click(t);
    }
  }

  function rowClick(k: Node) {
    if (k.insightId) {
      onToggleFinding?.(k.insightId);
    } else if (k.isDir) {
      otherOpen = false;
      onDrill?.(k.id);
    }
  }

  function dimmed(t: Tile): boolean {
    if (focusFinding) return t.findingId !== focusFinding;
    if (focusId) {
      if (focusId === OTHER_KEY) return !t.other;
      return t.key !== focusId;
    }
    const h = hovered;
    if (!h) return false;
    return h.key !== t.key;
  }

  const displayPct = $derived(
    hovered ? Math.min(100, (hovered.node.size / (totalSize || 1)) * 100) : 0
  );

  function labelSize(t: Tile): number {
    if (t.h >= 64) return 14;
    if (t.h >= 52) return 13;
    return 12.5;
  }

  function charWidth(ch: string, fs: number): number {
    return /[⺀-鿿　-〿＀-￯]/.test(ch) ? fs : fs * 0.58;
  }

  function rowLabel(t: Tile): { fs: number; baseY: number; name: string } {
    const fs = labelSize(t);
    const cy = t.y + t.h / 2;
    const baseY = cy + fs * 0.36;
    const nameX = t.x + 10;
    const rightPad = 10;
    const sizeStr = formatSize(t.node.size);
    let sizeW = 0;
    for (const ch of sizeStr) sizeW += charWidth(ch, fs * 0.92);
    sizeW += 2;

    const raw = t.other ? `其他 · ${t.count} 项` : t.node.name;
    const budget = t.x + t.w - nameX - rightPad - sizeW;
    let name = '';
    let used = 0;
    for (const ch of raw) {
      const wdt = charWidth(ch, fs);
      if (used + wdt > budget - fs * 0.5) {
        name = name.trimEnd() + '…';
        break;
      }
      name += ch;
      used += wdt;
    }
    return { fs, baseY, name };
  }

  function riskColor(risk?: Risk, chosen = false): string {
    if (chosen) return 'var(--color-accent)';
    if (risk === 'safe') return 'oklch(0.72 0.15 150)';
    if (risk === 'review') return 'oklch(0.78 0.13 75)';
    return 'var(--color-muted)';
  }

  function panelIn(_el: Element) {
    return {
      duration: 320,
      easing: backOut,
      css: (t: number) => `opacity:${t.toFixed(3)};transform:scale(${(0.92 + 0.08 * t).toFixed(3)})`
    };
  }

  function synthNode(name: string, size: number): Node {
    return {
      id: OTHER_KEY,
      parentId: null,
      name,
      path: '',
      isDir: false,
      size,
      modifiedMs: null,
      deletable: false,
      pending: false
    };
  }
</script>

<div class="relative h-full w-full" bind:this={stage}>
  <svg width={W} height={H} role="img" aria-label="磁盘空间方块面积图">
    <defs>
      {#each CATS as c, i}
        <linearGradient id="tm-g-{i}" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stop-color={`color-mix(in oklch, ${c} 88%, white 12%)`} />
          <stop offset="100%" stop-color={`color-mix(in oklch, ${c} 92%, black 14%)`} />
        </linearGradient>
      {/each}
      <linearGradient id="tm-g-other" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0%" stop-color="oklch(0.42 0.012 255)" />
        <stop offset="100%" stop-color="oklch(0.3 0.01 255)" />
      </linearGradient>
    </defs>

    <g>
      {#each tiles as t (t.key)}
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <rect
          x={t.x}
          y={t.y}
          width={t.w}
          height={t.h}
          style={rectStyle(t)}
          opacity={dimmed(t) ? 0.28 : 1}
          role={t.other || t.findingId || t.hasChildren ? 'button' : undefined}
          tabindex={t.other || t.findingId || t.hasChildren ? 0 : undefined}
          aria-label={
            t.other
              ? `其他 ${t.count} 项，共 ${formatSize(t.node.size)}，查看明细`
              : t.findingId
                ? `${t.node.name}，${formatSize(t.node.size)}，${isChosen(t) ? '已选，点击移出' : '点击加入清理'}`
                : t.hasChildren
                  ? `${t.node.name}，${formatSize(t.node.size)}，下钻`
                  : undefined
          }
          onclick={() => click(t)}
          oncontextmenu={(e) => openContextMenu(t, e)}
          onkeydown={(e) => onKeyDown(t, e)}
          onmousemove={(e) => {
            hoverKey = t.key;
            const r = stage.getBoundingClientRect();
            mouse = { x: e.clientX - r.left, y: e.clientY - r.top };
            onHoverId?.(t.other ? OTHER_KEY : t.node.id);
          }}
          onmouseleave={() => {
            hoverKey = null;
            onHoverId?.(null);
          }}
        ></rect>

        {#if t.h >= MIN_H - 2 && t.w >= MIN_W - 2}
          {@const lbl = rowLabel(t)}
          {@const nameFill = t.other ? 'oklch(0.85 0.01 255 / 0.92)' : 'oklch(0.2 0.02 255 / 0.92)'}
          {@const sizeFill = t.other ? 'oklch(0.72 0.01 255 / 0.85)' : 'oklch(0.2 0.02 255 / 0.66)'}

          <text
            x={t.x + 10}
            y={lbl.baseY}
            class="pointer-events-none select-none"
            fill={nameFill}
            style={`font-size: ${lbl.fs}px; font-weight: 650; letter-spacing: -0.01em`}
          >
            {lbl.name}
          </text>
          <text
            x={t.x + t.w - 10}
            y={lbl.baseY}
            text-anchor="end"
            class="num pointer-events-none select-none"
            fill={sizeFill}
            style={`font-size: ${(lbl.fs * 0.92).toFixed(1)}px; font-weight: 600`}
          >
            {formatSize(t.node.size)}
          </text>
        {/if}
      {/each}
    </g>
  </svg>

  {#if hovered && !otherOpen && !ctx}
    <div
      class="glass pointer-events-none absolute z-10 rounded-xl px-3 py-2"
      style="left: {Math.min(mouse.x + 16, W - 180)}px; top: {Math.min(mouse.y + 16, H - 82)}px; animation: tip-in 0.14s ease both"
    >
      <div class="max-w-[210px] truncate text-[12px] font-[650]">
        {#if hovered.other}其他 · {hovered.count} 项{:else}{hovered.node.name}{/if}
      </div>
      <div class="num text-[11px]" style="color: var(--color-muted)">
        {formatSize(hovered.node.size)} · {displayPct.toFixed(1)}%
      </div>
      {#if hovered.other}
        <div class="mt-0.5 text-[10.5px]" style="color: var(--color-accent-hi)">点击查看明细</div>
      {:else if hovered.node.note}
        <div class="mt-0.5 flex items-center gap-1 text-[10.5px]" style="color: var(--color-accent-hi)">
          <Icon name="spark" size={10} /> {hovered.node.note}
        </div>
      {/if}
    </div>
  {/if}

  {#if ctx}
    <div
      class="ctx-menu absolute z-40 overflow-hidden rounded-xl py-1"
      style="left: {ctx.x}px; top: {ctx.y}px; width: {MENU_W}px"
      role="menu"
      in:scale={{ duration: 130, start: 0.96 }}
      out:scale={{ duration: 110, start: 0.96, opacity: 0 }}
    >
      <button
        type="button"
        role="menuitem"
        class="ctx-item flex w-full items-center gap-2.5 px-3 py-2 text-left text-[12.5px]"
        class:ctx-item-off={ctx.tile.node.deletable === false}
        disabled={ctx.tile.node.deletable === false}
        onclick={confirmContextAdd}
        data-od-id="ctx-add-delete"
      >
        <Icon name="trash" size={14} class="shrink-0" />
        <span class="min-w-0 flex-1">添加到删除列表</span>
        {#if ctx.tile.node.deletable === false}
          <span class="shrink-0 text-[10.5px]" style="color: var(--color-faint)">无权限</span>
        {/if}
      </button>
    </div>
  {/if}

  {#if otherOpen}
    <button
      class="absolute inset-0 z-20 cursor-default"
      style="background: oklch(0.14 0.012 255 / 0.45); backdrop-filter: blur(6px); border: 0"
      aria-label="关闭其他明细"
      onclick={() => (otherOpen = false)}
    ></button>
    <div
      class="glass-strong absolute z-30 flex flex-col overflow-hidden rounded-2xl"
      style="left: 5%; top: 7%; width: 90%; height: 86%"
      role="dialog"
      tabindex="-1"
      aria-modal="true"
      aria-label="其他文件明细"
      in:panelIn
      out:fade={{ duration: 160 }}
      onkeydown={(e) => e.key === 'Escape' && (otherOpen = false)}
    >
      <header class="flex items-center gap-3 px-5 pb-3 pt-4">
        <div class="min-w-0 flex-1">
          <div class="text-[15px] font-[680] tracking-[-0.01em]">
            其他 <span class="num" style="color: var(--color-muted)">{foldedKids.length} 项</span>
          </div>
          <div class="num mt-0.5 text-[11.5px]" style="color: var(--color-muted)">
            共 {formatSize(otherBytes)} · 小于方块最小显示尺寸，已聚合
          </div>
        </div>
        <div class="search-box flex items-center gap-2 rounded-lg px-2.5 py-1.5">
          <Icon name="search" size={13} />
          <input
            bind:value={query}
            type="text"
            placeholder="筛选文件…"
            class="w-[150px] bg-transparent text-[12px] outline-none placeholder:text-[color:var(--color-muted)]"
          />
        </div>
        <button class="icon-btn" aria-label="关闭" onclick={() => (otherOpen = false)}>
          <Icon name="x" size={15} />
        </button>
      </header>

      <div class="mx-5 mb-3 h-px" style="background: var(--color-border)"></div>

      <div class="min-h-0 flex-1 overflow-y-auto px-3 pb-4">
        {#each otherRows as k, i (k.id)}
          {@const chosen = !!k.insightId && selectedIds.has(k.insightId)}
          {@const canDelete = k.deletable !== false && k.risk !== 'keep'}
          <div
            class="other-row w-full rounded-xl px-2.5 py-2"
            style="animation-delay: {Math.min(i * 18, 360)}ms"
            role="button"
            tabindex="0"
            onclick={() => rowClick(k)}
            onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), rowClick(k))}
          >
            <div class="flex items-center gap-2.5">
              <span class="shrink-0" style={`color: ${riskColor(k.risk, chosen)}`}>
                <Icon name={k.isDir ? 'folder' : k.risk === 'keep' ? 'shield' : 'file'} size={15} />
              </span>
              <span class="min-w-0 flex-1 truncate text-[12.5px] font-[560]">{k.name}</span>
              <span class="num shrink-0 text-[11px]" style="color: var(--color-muted)">
                {formatSize(k.size)}
              </span>
              {#if canDelete}
                <button
                  type="button"
                  class="row-del-btn shrink-0"
                  aria-label={chosen ? '从删除队列移除' : '添加到删除队列'}
                  title={chosen ? '从删除队列移除' : '添加到删除队列'}
                  onclick={(e) => {
                    e.stopPropagation();
                    if (k.insightId) onToggleFinding?.(k.insightId);
                    else onAddToDelete?.(k);
                  }}
                >
                  <Icon name={chosen ? 'undo' : 'trash'} size={13} />
                </button>
              {/if}
            </div>
            <div class="mt-1.5 h-[3px] overflow-hidden rounded-full" style="background: var(--color-border)">
              <div
                class="h-full rounded-full"
                style="width: {Math.max(3, (k.size / otherMaxSize) * 100)}%; background: {riskColor(k.risk, chosen)}"
              ></div>
            </div>
          </div>
        {:else}
          <div class="px-3 py-10 text-center text-[12.5px]" style="color: var(--color-muted)" in:scale={{ duration: 180 }}>
            没有匹配「{query}」的项目
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  @keyframes tip-in {
    from {
      opacity: 0;
      transform: translateY(3px);
    }
  }
  .other-row {
    animation: row-in 0.34s cubic-bezier(0.22, 1, 0.36, 1) both;
    transition: background 0.15s ease;
  }
  .other-row:hover {
    background: oklch(0.98 0.005 250 / 0.055);
  }
  .row-del-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border-radius: 6px;
    border: 0;
    background: transparent;
    color: var(--color-faint);
    cursor: pointer;
    transition: background 0.14s ease, color 0.14s ease;
  }
  .row-del-btn:hover {
    background: color-mix(in oklch, var(--color-danger, oklch(0.62 0.2 25)) 18%, transparent);
    color: oklch(0.78 0.16 25);
  }
  .row-del-btn:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: 1px;
  }
  @keyframes row-in {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }
  .search-box {
    background: oklch(0.98 0.005 250 / 0.05);
    border: 1px solid var(--color-border);
  }
  .search-box:focus-within {
    border-color: oklch(0.7 0.12 250 / 0.6);
  }
  .icon-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 30px;
    height: 30px;
    border-radius: 8px;
    color: var(--color-muted);
    border: 0;
    background: transparent;
    transition: background 0.15s ease, color 0.15s ease;
  }
  .icon-btn:hover {
    background: oklch(0.98 0.005 250 / 0.07);
    color: var(--color-fg);
  }
  .icon-btn:focus-visible {
    outline: 2px solid var(--color-accent);
    outline-offset: 2px;
  }

  .ctx-menu {
    background: color-mix(in oklch, var(--color-surface) 88%, var(--color-bg));
    border: 1px solid var(--color-border-strong);
    box-shadow:
      0 2px 8px -2px oklch(0% 0 0 / 0.5),
      0 18px 44px -12px oklch(0% 0 0 / 0.6);
    transform-origin: top left;
  }
  .ctx-item {
    color: var(--color-fg);
    transition: background 0.13s ease, color 0.13s ease;
  }
  .ctx-item:hover:not(:disabled) {
    background: color-mix(in oklch, var(--color-accent) 16%, var(--color-surface));
  }
  .ctx-item:focus-visible {
    outline: none;
    box-shadow: inset 0 0 0 2px var(--color-accent);
  }
  .ctx-item.ctx-item-off {
    color: var(--color-muted);
    cursor: not-allowed;
  }
</style>

<script lang="ts">
  import { backOut, cubicOut } from 'svelte/easing';
  import { fade, scale } from 'svelte/transition';
  import { formatSize } from '../format';
  import type { FileNode } from '../types';
  import Icon from './Icon.svelte';

  let {
    entries,
    totalSize = 0,
    pending = false,
    focusNodeId = null,
    onHoverId,
    onDrill,
    onContextNode
  }: {
    entries: FileNode[];
    totalSize?: number;
    pending?: boolean;
    focusNodeId?: string | null;
    onHoverId?: (id: string | null) => void;
    onDrill?: (id: string) => void;
    onContextNode?: (node: FileNode, clientX: number, clientY: number) => void;
  } = $props();

  const OTHER_ID = '__other__';

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
    node: FileNode | null;
    colorIdx: number;
    other?: boolean;
    count?: number;
  }

  // ---------- measured stage size ----------
  let stage: HTMLDivElement;
  let W = $state(800);
  let H = $state(600);

  $effect(() => {
    if (!stage) return;
    const ro = new ResizeObserver((es) => {
      const r = es[0].contentRect;
      W = Math.max(120, r.width);
      H = Math.max(120, r.height);
    });
    ro.observe(stage);
    return () => ro.disconnect();
  });

  // ---------- squarified treemap ----------
  interface V {
    v: number;
    node: FileNode | null;
  }

  function worst(vals: number[], cx: number, cy: number, cw: number, ch: number, total: number): number {
    const scale = (cw * ch) / total;
    const horizontal = cw >= ch;
    const fixed = horizontal ? cw : ch;
    const sum = vals.reduce((a, b) => a + b, 0);
    const t = (sum * scale) / fixed;
    let worstR = 0;
    for (const v of vals) {
      const d = (v * scale) / t;
      const r = Math.max(d / t, t / d);
      if (r > worstR) worstR = r;
    }
    return worstR;
  }

  function squarify(items: V[], area: Box, total: number): { box: Box; node: FileNode | null }[] {
    const out: { box: Box; node: FileNode | null }[] = [];
    let { x: cx, y: cy, w: cw, h: ch } = area;
    let row: V[] = [];
    let i = 0;
    let rem = total;

    const flush = () => {
      const sum = row.reduce((a, b) => a + b.v, 0);
      const scale = (cw * ch) / rem;
      const horizontal = cw >= ch;
      const fixed = horizontal ? cw : ch;
      const t = (sum * scale) / fixed;
      if (horizontal) {
        let cur = cx;
        for (const it of row) {
          const d = (it.v * scale) / t;
          out.push({ box: { x: cur, y: cy, w: d, h: t }, node: it.node });
          cur += d;
        }
        cy += t;
        ch -= t;
      } else {
        let cur = cy;
        for (const it of row) {
          const d = (it.v * scale) / t;
          out.push({ box: { x: cx, y: cur, w: t, h: d }, node: it.node });
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
        const before = worst(row.map((r) => r.v), cx, cy, cw, ch, rem);
        const after = worst([...row.map((r) => r.v), item.v], cx, cy, cw, ch, rem);
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
    flush();
    return out;
  }

  const PAD = 14;
  const MIN_W = 88;
  const MIN_H = 48;

  let hoverKey = $state<string | null>(null);
  let otherOpen = $state(false);
  let query = $state('');

  const layout = $derived.by<{ tiles: Tile[]; folded: FileNode[] }>(() => {
    void W;
    void H;
    const kids = [...entries].sort((a, b) => b.size - a.size);

    const full: Box = {
      x: PAD,
      y: PAD,
      w: Math.max(80, W - PAD * 2),
      h: Math.max(80, H - PAD * 2)
    };
    const stageArea = full.w * full.h;

    const buildTile = (raw: Box, node: FileNode | null, i: number, other = false, count?: number): Tile => ({
      x: raw.x,
      y: raw.y,
      w: raw.w,
      h: raw.h,
      key: other ? OTHER_ID : node!.id,
      node,
      colorIdx: i % CATS.length,
      other,
      count
    });

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
      let gLo = lo;
      for (let k = 0; k < 22; k++) {
        const mid = (gLo + hi) / 2;
        if (feasible(mid)) gLo = mid;
        else break;
      }
      return gLo;
    };

    let nReal = kids.length;
    for (let guard = 0; guard <= kids.length + 1; guard++) {
      const realKids = kids.slice(0, nReal);
      const folded = kids.slice(nReal);
      const foldedRealSize = folded.reduce((s, k) => s + k.size, 0);
      const gamma = chooseGamma(realKids.map((k) => k.size));
      const values: V[] = realKids.map((n) => ({ v: n.size ** gamma, node: n }));
      if (folded.length) {
        const foldedWeight = folded.reduce((s, k) => s + k.size ** gamma, 0);
        values.push({ v: foldedWeight, node: null });
      }
      const weightTotal = values.reduce((s, v) => s + v.v, 0) || 1;
      const laid = squarify(values, full, weightTotal);
      const tiles: Tile[] = [];
      let underSized = false;

      laid.forEach(({ box: raw, node }, i) => {
        const isOther = !!folded.length && i === laid.length - 1;
        if (raw.w < MIN_W || raw.h < MIN_H) {
          underSized = true;
          return;
        }
        if (isOther) tiles.push(buildTile(raw, null, i, true, folded.length));
        else tiles.push(buildTile(raw, node, i));
      });

      if (underSized) {
        if (nReal === 0) {
          return {
            tiles: [buildTile(full, null, 0, true, kids.length)],
            folded: kids
          };
        }
        nReal -= 1;
        continue;
      }
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

  const tileByKey = $derived(new Map(tiles.map((t) => [t.key, t])));
  const hovered = $derived(hoverKey ? (tileByKey.get(hoverKey) ?? null) : null);

  function isFocusTile(t: Tile): boolean {
    if (focusNodeId) return focusNodeId === OTHER_ID ? !!t.other : t.key === focusNodeId;
    return hoverKey === t.key;
  }

  function tileFill(t: Tile): string {
    return t.other ? 'var(--color-cat-other)' : CATS[t.colorIdx];
  }

  function rectStyle(t: Tile): string {
    const isHover = isFocusTile(t);
    return [
      `cursor: ${t.other || t.node?.isDir ? 'pointer' : 'default'}`,
      `fill: ${tileFill(t)}`,
      `stroke: ${isHover ? 'oklch(0.985 0.005 250)' : 'oklch(0.13 0.01 255 / 0.55)'}`,
      'stroke-width: 1.5px',
      `filter: brightness(${isHover ? 1.12 : 1})`,
      'transition: stroke 0.16s ease, filter 0.16s ease, opacity 0.18s ease'
    ].join(';');
  }

  function clickTile(t: Tile) {
    if (t.other) {
      otherOpen = true;
      query = '';
      return;
    }
    if (t.node?.isDir) onDrill?.(t.node.id);
  }

  function onTileKey(t: Tile, event: KeyboardEvent) {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      clickTile(t);
    }
  }

  function dimmed(t: Tile): boolean {
    if (focusNodeId) {
      return focusNodeId === OTHER_ID ? !t.other : t.key !== focusNodeId;
    }
    const h = hovered;
    if (!h) return false;
    return h.key !== t.key;
  }

  function labelSize(t: Tile): number {
    if (t.h >= 64) return 14;
    if (t.h >= 52) return 13;
    return 12.5;
  }

  function charWidth(ch: string, fs: number): number {
    return /[⺀-鿿　-〿＀-￯]/.test(ch) ? fs : fs * 0.58;
  }

  /** Single-row label: name ........ size, truncated to fit. */
  function rowLabel(t: Tile): { baseY: number; name: string; fs: number } {
    const fs = labelSize(t);
    const baseY = t.y + t.h / 2 + fs * 0.36;
    const nameX = t.x + 10;
    const sizeNode = t.other ? null : t.node;
    const sizeStr = t.other ? '' : formatSize(sizeNode!.size);
    let sizeW = 0;
    for (const ch of sizeStr) sizeW += charWidth(ch, fs * 0.92);

    const raw = t.other ? `其他 · ${t.count} 项` : sizeNode!.name;
    const budget = t.x + t.w - nameX - 10 - sizeW;
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
    return { baseY, name, fs };
  }

  function panelIn(_el: Element) {
    return {
      duration: 320,
      easing: backOut,
      css: (t: number) => `opacity:${t.toFixed(3)};transform:scale(${(0.92 + 0.08 * t).toFixed(3)})`
    };
  }
</script>

<div class="relative h-full w-full" bind:this={stage}>
  <!-- pending sweep hint -->
  {#if pending}
    <div class="absolute left-3 top-2 z-10 flex items-center gap-1.5 text-[10.5px]" style="color: var(--color-faint)">
      <span class="inline-block h-3 w-3 animate-spin"
        ><Icon name="refresh" size={11}
      /></span>
      正在扫描…
    </div>
  {/if}

  <svg width={W} height={H} class="block" role="img" aria-label="磁盘占用方块图">
    <defs>
      {#each CATS as _, i}
        <linearGradient id={`tm-live-${i}`} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stop-color={CATS[i]} stop-opacity="0.92" />
          <stop offset="100%" stop-color={CATS[i]} stop-opacity="0.62" />
        </linearGradient>
      {/each}
    </defs>

    {#each tiles as t (t.key)}
      <!-- eslint a11y: interactive rect mirrors a keyboard-accessible button below -->
      <rect
        x={t.x}
        y={t.y}
        width={t.w}
        height={t.h}
        rx="0"
        style={rectStyle(t)}
        opacity={dimmed(t) ? 0.3 : 1}
        role="button"
        tabindex="0"
        aria-label={t.other ? `其他 ${t.count} 项` : t.node!.name}
        onclick={() => clickTile(t)}
        oncontextmenu={(e: MouseEvent) => {
          if (!t.other && t.node) {
            e.preventDefault();
            onContextNode?.(t.node, e.clientX, e.clientY);
          }
        }}
        onkeydown={(e) => onTileKey(t, e)}
        onmouseenter={() => {
          hoverKey = t.key;
          onHoverId?.(t.other ? OTHER_ID : t.node!.id);
        }}
        onmouseleave={() => {
          hoverKey = null;
          onHoverId?.(null);
        }}
      ></rect>
      {#if t.w >= 70 && t.h >= 26}
        {@const lbl = rowLabel(t)}
        <text
          x={t.x + 10}
          y={lbl.baseY}
          font-size={lbl.fs}
          font-weight="600"
          fill="oklch(0.97 0.004 255 / 0.92)"
          style="pointer-events:none;paint-order:stroke"
          stroke="oklch(0.12 0.01 255 / 0.55)"
          stroke-width="2.5"
        >
          {lbl.name}
        </text>
        {#if !t.other}
          <text
            x={t.x + t.w - 10}
            y={lbl.baseY}
            font-size={lbl.fs * 0.92}
            text-anchor="end"
            fill="oklch(0.97 0.004 255 / 0.75)"
            style="pointer-events:none;paint-order:stroke"
            stroke="oklch(0.12 0.01 255 / 0.5)"
            stroke-width="2.5"
          >
            {formatSize(t.node!.size)}
          </text>
        {/if}
      {/if}
    {/each}
  </svg>

  {#if otherOpen}
    <!-- immersive aggregate panel for the long-tail "其他" group -->
    <button
      type="button"
      aria-label="关闭"
      class="absolute inset-0 z-20"
      style="background: oklch(0.08 0.01 255 / 0.45); backdrop-filter: blur(3px)"
      onclick={() => (otherOpen = false)}
    ></button>
    <div
      class="glass-strong absolute inset-x-6 bottom-6 top-16 z-30 flex flex-col rounded-2xl"
      role="dialog"
      aria-label="其他项目"
      in:panelIn
      out:scale={{ duration: 160, start: 0.96, opacity: 0.5 }}
    >
      <div class="flex items-center gap-3 px-4 pt-3.5">
        <h2 class="text-[14px] font-[650]">其他 · {foldedKids.length} 项</h2>
        <span class="num text-[11.5px]" style="color: var(--color-faint)"
          >{formatSize(foldedKids.reduce((s, k) => s + k.size, 0))}</span
        >
        <div class="relative ml-auto w-[200px]">
          <input
            type="text"
            bind:value={query}
            placeholder="搜索"
            class="h-8 w-full rounded-lg pl-8 pr-3 text-[12px] outline-none"
            style="
              background: color-mix(in oklch, var(--color-bg) 60%, transparent);
              border: 1px solid var(--color-border);
              color: var(--color-fg);
            "
          />
          <Icon
            name="search"
            size={13}
            class="absolute left-2.5 top-1/2 -translate-y-1/2"
            style="color: var(--color-faint)"
          />
        </div>
        <button
          type="button"
          aria-label="关闭"
          class="btn-icon"
          onclick={() => (otherOpen = false)}
        >
          <Icon name="x" size={15} />
        </button>
      </div>

      <ul class="mt-2 min-h-0 flex-1 overflow-y-auto px-3 pb-3">
        {#each otherRows as k, i (k.id)}
          <li
            class="flex items-center gap-3 rounded-lg px-2.5 py-2"
            style="animation: other-row-in 0.26s cubic-bezier(0.22,1,0.36,1) both; animation-delay: {Math.min(i, 10) * 22}ms"
          >
            <span class="flex w-[18px] justify-center" style="color: var(--color-faint)">
              <Icon name={k.isDir ? 'folder' : 'hardDrive'} size={14} />
            </span>
            <button
              type="button"
              class="flex min-w-0 flex-1 items-center text-left"
              onclick={() => {
                if (k.isDir) {
                  otherOpen = false;
                  onDrill?.(k.id);
                }
              }}
            >
              <span class="truncate text-[12.5px] {k.isDir ? 'font-[580]' : ''}">{k.name}</span>
            </button>
            <span class="h-1.5 w-[140px] overflow-hidden rounded-full" style="background: var(--color-surface-3)">
              <span
                class="block h-full rounded-full"
                style="width: {Math.max(4, (k.size / otherMaxSize) * 100)}%; background: var(--color-cat-8)"
              ></span>
            </span>
            <span class="num w-[72px] shrink-0 text-right text-[11.5px]" style="color: var(--color-muted)"
              >{formatSize(k.size)}</span
            >
          </li>
        {/each}
        {#if otherRows.length === 0}
          <li class="py-14 text-center text-[12px]" style="color: var(--color-faint)">没有匹配的项目</li>
        {/if}
      </ul>
    </div>
  {/if}
</div>

<style>
  @keyframes other-row-in {
    from {
      opacity: 0;
      transform: translateX(10px);
    }
  }
</style>

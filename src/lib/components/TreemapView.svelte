<script lang="ts">
  import Icon from './Icon.svelte';
  import { store } from '../store.svelte';
  import { formatSize } from '../format';
  import { t } from '../i18n.svelte';
  import { layoutTreemap, type TreemapItem } from '../treemap-layout';
  import type { Node } from '../types';

  const OTHER_ID = '__other__';

  // ---- stage measurement ----
  let stage = $state<HTMLDivElement | null>(null);
  let stageSize = $state({ w: 0, h: 0 });
  let ro: ResizeObserver | null = null;

  $effect(() => {
    if (!stage) return;
    ro ??= new ResizeObserver(() => {
      stageSize.w = stage!.clientWidth;
      stageSize.h = stage!.clientHeight;
    });
    ro.observe(stage);
    stageSize.w = stage.clientWidth;
    stageSize.h = stage.clientHeight;
    return () => ro?.disconnect();
  });

  // ---- model: live entries + subpixel aggregation ----
  const model = $derived.by(() => {
    const entries = store.listEntries;
    const totalBytes = entries.reduce((sum, e) => sum + Math.max(0, e.size), 0);
    const totalArea = stageSize.w * stageSize.h;
    // A cell whose area would be smaller than a 1px strip on the short edge
    // cannot render meaningfully; fold such entries into one "other" cell.
    const minArea = Math.min(stageSize.w, stageSize.h);
    const items: TreemapItem[] = [];
    const small: Node[] = [];
    for (const e of entries) {
      if (e.size <= 0) continue;
      const area = totalBytes > 0 ? (e.size / totalBytes) * totalArea : 0;
      if (area < minArea) small.push(e);
      else items.push({ id: e.id, weight: e.size });
    }
    const otherBytes = small.reduce((sum, e) => sum + e.size, 0);
    if (otherBytes > 0) items.push({ id: OTHER_ID, weight: otherBytes });
    return { items, small, otherBytes };
  });

  const rects = $derived(
    layoutTreemap(model.items, { x: 0, y: 0, w: stageSize.w, h: stageSize.h })
  );

  const entryById = $derived(new Map(store.listEntries.map((e) => [e.id, e])));

  // ---- hover / popover ----
  let hoverId = $state<string | null>(null);
  const hover = $derived(hoverId ? (entryById.get(hoverId) ?? null) : null);
  let otherOpen = $state(false);
  let filterText = $state('');
  const filteredSmall = $derived(
    filterText.trim()
      ? model.small.filter((e) => e.name.toLowerCase().includes(filterText.trim().toLowerCase()))
      : model.small
  );

  // ---- actions ----
  function clickCell(id: string) {
    if (id === OTHER_ID) {
      otherOpen = !otherOpen;
      return;
    }
    const entry = entryById.get(id);
    if (!entry) return;
    if (entry.insightId) store.toggleSelected(entry.insightId);
    else store.addManualCandidate(entry);
  }

  function drillCell(id: string) {
    if (id === OTHER_ID) {
      otherOpen = true;
      return;
    }
    const entry = entryById.get(id);
    if (entry?.isDir) store.drillInto(entry.name);
  }

  function ariaLabel(id: string): string {
    if (id === OTHER_ID) {
      return t('treemap.otherAria', [String(model.small.length), formatSize(model.otherBytes)]);
    }
    const entry = entryById.get(id);
    if (!entry) return '';
    const riskPart =
      entry.risk === 'safe'
        ? t('treemap.safe')
        : entry.risk === 'review'
          ? t('treemap.review')
          : '';
    const base = t('treemap.ariaFinding', [entry.name, formatSize(entry.size), riskPart]);
    return entry.isDir ? base + t('treemap.ariaFolder') : base;
  }
</script>

<div class="tm-host">
  <div class="tm-stage" bind:this={stage} role="img" aria-label={t('treemap.aria')}>
    {#if rects.length === 0}
      <div class="tm-empty">
        <Icon name="dashboard" size={22} style="color: var(--color-faint)" />
        <p>{store.scanning ? t('treemap.aria') : t('list.folderEmpty')}</p>
      </div>
    {/if}

    {#each rects as r (r.id)}
      {@const entry = r.id === OTHER_ID ? null : (entryById.get(r.id) ?? null)}
      {@const selected = entry?.insightId ? store.isSelected(entry.insightId) : false}
      <button
        type="button"
        class="tm-cell risk-{entry?.risk ?? (r.id === OTHER_ID ? 'other' : 'plain')}"
        class:cell-selected={selected}
        style="left:{r.x}px;top:{r.y}px;width:{r.w}px;height:{r.h}px"
        aria-label={ariaLabel(r.id)}
        onclick={() => clickCell(r.id)}
        ondblclick={() => drillCell(r.id)}
        onmouseenter={() => (hoverId = r.id)}
        onmouseleave={() => (hoverId = null)}
      >
        <span class="tm-inner">
          {#if r.w > 58 && r.h > 20}
            <span class="tm-label">
              {#if r.id === OTHER_ID}
                <span class="tm-name">{t('treemap.otherN', [String(model.small.length)])}</span>
              {:else}
                <Icon name={entry?.isDir ? 'folder' : 'hardDrive'} size={11} />
                <span class="tm-name">{entry?.name}</span>
              {/if}
            </span>
          {/if}
          {#if r.w > 58 && r.h > 40}
            <span class="tm-bytes num">
              {r.id === OTHER_ID ? formatSize(model.otherBytes) : formatSize(entry?.size ?? 0)}
            </span>
          {/if}
        </span>
      </button>
    {/each}
  </div>

  {#if hover && !otherOpen}
    <div class="tm-hover" aria-hidden="true">
      <Icon name={hover.isDir ? 'folder' : 'hardDrive'} size={12} />
      <span class="tm-hover-name">{hover.name}</span>
      <span class="num">{formatSize(hover.size)}</span>
      <span class="tm-hover-hint">
        {hover.isDir ? t('treemap.ariaFolder') : t('treemap.clickClean')}
      </span>
    </div>
  {/if}

  {#if otherOpen}
    <div class="tm-pop" role="dialog" aria-label={t('treemap.otherDetail')}>
      <header class="tm-pop-head">
        <p class="tm-pop-title">{t('treemap.otherNItems', [String(model.small.length)])}</p>
        <button
          type="button"
          class="tm-pop-close"
          aria-label={t('treemap.closeOther')}
          onclick={() => (otherOpen = false)}
        >
          <Icon name="x" size={13} />
        </button>
      </header>
      <p class="tm-pop-bytes">{t('treemap.otherBytes', [formatSize(model.otherBytes)])}</p>

      <div class="tm-pop-filter">
        <Icon name="search" size={12} />
        <input bind:value={filterText} placeholder={t('treemap.filterPlaceholder')} />
      </div>

      <ul class="tm-pop-list">
        {#each filteredSmall as entry (entry.id)}
          <li>
            <Icon name={entry.isDir ? 'folder' : 'hardDrive'} size={12} />
            <span class="tm-pop-name">{entry.name}</span>
            <span class="num tm-pop-size">{formatSize(entry.size)}</span>
            <button type="button" class="tm-pop-add" onclick={() => store.addManualCandidate(entry)}>
              <Icon name="trash" size={11} />
            </button>
          </li>
        {:else}
          <li class="tm-pop-nomatch">{t('treemap.noMatch', [filterText])}</li>
        {/each}
      </ul>
    </div>
  {/if}
</div>

<style>
  .tm-host {
    position: relative;
    height: 100%;
    min-height: 0;
  }
  .tm-stage {
    position: relative;
    height: 100%;
    overflow: hidden;
    border-radius: 12px;
  }
  .tm-cell {
    position: absolute;
    padding: 0;
    border: none;
    background: transparent;
    cursor: default;
  }
  .tm-inner {
    position: absolute;
    inset: 1.5px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 6px 8px;
    overflow: hidden;
    border: 1px solid var(--color-border);
    border-radius: 7px;
    background: var(--color-surface-2);
    text-align: left;
  }
  .tm-cell:focus-visible .tm-inner {
    outline: 2px solid var(--color-violet);
    outline-offset: 1px;
  }
  .risk-safe .tm-inner {
    background: color-mix(in oklch, var(--color-ok) 11%, var(--color-surface-2));
    border-color: color-mix(in oklch, var(--color-ok) 32%, var(--color-border));
  }
  .risk-review .tm-inner {
    background: color-mix(in oklch, var(--color-warn) 11%, var(--color-surface-2));
    border-color: color-mix(in oklch, var(--color-warn) 32%, var(--color-border));
  }
  .risk-other .tm-inner {
    border-style: dashed;
    color: var(--color-faint);
  }
  .cell-selected .tm-inner {
    background: color-mix(in oklch, var(--color-violet) 20%, var(--color-surface-2));
    border-color: color-mix(in oklch, var(--color-violet) 55%, var(--color-border));
  }
  .tm-label {
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
    color: var(--color-fg);
  }
  .tm-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 11.5px;
    font-weight: 600;
  }
  .tm-bytes {
    font-size: 10.5px;
    color: var(--color-muted);
  }
  .tm-empty {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    font-size: 12px;
    color: var(--color-faint);
  }
  .tm-hover {
    position: absolute;
    left: 10px;
    bottom: 10px;
    display: flex;
    align-items: center;
    gap: 8px;
    max-width: calc(100% - 20px);
    padding: 7px 11px;
    border: 1px solid var(--color-border);
    border-radius: 9px;
    background: color-mix(in oklch, var(--color-surface-2) 92%, var(--color-bg));
    box-shadow: 0 10px 26px -12px oklch(0% 0 0 / 0.6);
    font-size: 11px;
    color: var(--color-muted);
    pointer-events: none;
  }
  .tm-hover-name {
    font-weight: 620;
    color: var(--color-fg);
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tm-hover-hint {
    color: var(--color-faint);
    font-size: 10px;
  }
  .tm-pop {
    position: absolute;
    right: 10px;
    bottom: 10px;
    display: flex;
    flex-direction: column;
    width: min(340px, calc(100% - 20px));
    max-height: calc(100% - 20px);
    padding: 12px;
    border: 1px solid var(--color-border-strong);
    border-radius: 12px;
    background: color-mix(in oklch, var(--color-surface-2) 94%, var(--color-bg));
    box-shadow: 0 22px 48px -14px oklch(0% 0 0 / 0.65);
  }
  .tm-pop-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .tm-pop-title {
    margin: 0;
    font-size: 12.5px;
    font-weight: 640;
  }
  .tm-pop-close {
    display: inline-flex;
    border: none;
    background: transparent;
    color: var(--color-faint);
    cursor: default;
  }
  .tm-pop-bytes {
    margin: 4px 0 10px;
    font-size: 10.5px;
    color: var(--color-faint);
  }
  .tm-pop-filter {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 5px 9px;
    margin-bottom: 8px;
    border: 1px solid var(--color-border);
    border-radius: 8px;
    color: var(--color-faint);
  }
  .tm-pop-filter input {
    flex: 1;
    min-width: 0;
    border: none;
    outline: none;
    background: transparent;
    color: var(--color-fg);
    font-size: 11.5px;
  }
  .tm-pop-list {
    margin: 0;
    padding: 0;
    list-style: none;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
  }
  .tm-pop-list li {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 5px 4px;
    border-radius: 7px;
    color: var(--color-muted);
  }
  .tm-pop-list li:hover {
    background: color-mix(in oklch, var(--color-accent) 12%, transparent);
  }
  .tm-pop-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 11.5px;
  }
  .tm-pop-size {
    font-size: 10.5px;
  }
  .tm-pop-add {
    display: inline-flex;
    border: none;
    background: transparent;
    color: var(--color-faint);
    cursor: default;
  }
  .tm-pop-add:hover {
    color: var(--color-danger);
  }
  .tm-pop-nomatch {
    justify-content: center;
    font-size: 11px;
    color: var(--color-faint);
  }
</style>

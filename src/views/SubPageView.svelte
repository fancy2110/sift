<script lang="ts">
  import Icon from '../lib/components/Icon.svelte';
  import LocationPicker from '../lib/components/LocationPicker.svelte';
  import { store } from '../lib/store.svelte';
  import { formatSize } from '../lib/format';
  import type { Node } from '../lib/types';
  type SubTab = 'smart' | 'explorer' | 'history';
  import { fade, scale } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { t } from '../lib/i18n.svelte';
  import { kindTitle, reasonText, impactText, cleanupMethodText, cleanupCommandText } from '../lib/reasons';

  function selectAllSmart() {
    store.selectedIds = new Set(store.smartItems.map((i) => i.id));
  }
  function deselectAllSmart() {
    store.selectedIds = new Set();
  }

  async function confirmSift() {
    await store.clean(false);
  }

  let listCtx = $state<{ x: number; y: number; node: Node; inQueue: boolean } | null>(null);

  function openListContext(e: MouseEvent, node: Node) {
    e.preventDefault();
    const host = (e.currentTarget as HTMLElement).closest<HTMLElement>('[data-od-id="explorer-list"]');
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
      store.toast(t('list.removedFromQueue', [c.node.name]));
    } else {
      store.addManualCandidate(c.node);
    }
  }

  function ago(ts: number): string {
    const d = Math.round((Date.now() - ts) / 86400000);
    if (d <= 0) return t('time.today');
    if (d === 1) return t('time.yesterday');
    return t('time.daysAgo', [d]);
  }
  function clockAt(ts: number): string {
    return new Date(ts).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  }

  function listIn(_el: Element, { index, base = 0 }: { index: number; base?: number }) {
    return {
      duration: 300,
      delay: base + Math.min(index, 8) * 30,
      easing: cubicOut,
      css: (tm: number) =>
        `opacity:${tm};transform:translateY(${(12 * (1 - tm)).toFixed(1)}px) scale(${(0.99 + 0.01 * tm).toFixed(4)})`
    };
  }

  const tabIndex = { smart: 0, explorer: 1, history: 2 } as const;
  const pagerOffset = $derived(tabIndex[store.subTab] * (100 / 3));

  const TABS: [SubTab, string, string][] = [
    ['smart', 'spark', t('sub.tab.smart')],
    ['explorer', 'layers', t('sub.tab.explorer')],
    ['history', 'clock', t('sub.tab.history')]
  ];
</script>

<div class="subpage">
  <header class="sp-head">
    <button type="button" class="btn btn-quiet sp-back" onclick={() => store.goHome()} aria-label={t('common.back')}>
      <Icon name="chevronLeft" size={16} />
    </button>

    <span class="sp-total">
      <span class="sp-total-label">{t('sub.pending')}</span>
      <span class="num sp-total-big">{formatSize(store.selectedBytes)}</span>
    </span>

    <button
      type="button"
      class="sp-confirm"
      disabled={store.selectedIds.size === 0 || store.cleaning}
      onclick={confirmSift}
    >
      {#if store.cleaning}
        <span class="spin" style="display: inline-flex"><Icon name="refresh" size={14} /></span>
        {t('sub.cleaning')}
      {:else}
        <Icon name="trash" size={14} />
        {t('sub.confirm')}
      {/if}
    </button>
  </header>

  <div class="sp-tabbar">
    <div class="segmented" role="tablist" aria-label={t('sub.tabsLabel')}>
      <span
        class="segmented-thumb"
        style="width: calc(100%/3); transform: translateX({tabIndex[store.subTab] * 100}%)"
        aria-hidden="true"
      ></span>
      {#each TABS as [tabId, icon, label]}
        <button
          type="button"
          role="tab"
          aria-selected={store.subTab === tabId}
          class="seg-tab"
          class:seg-tab-on={store.subTab === tabId}
          onclick={() => store.setSubTab(tabId)}
        >
          <Icon name={icon} size={13.5} />
          {label}
        </button>
      {/each}
    </div>
  </div>

  <div class="sp-body">
    <div class="pager" style="transform: translateX(-{pagerOffset}%)">
      <!-- SMART -->
      <section class="pager-panel" role="tabpanel">
        <div class="smart-scroll">
          {#each store.smartItems as item, i (item.id)}
            {@const checked = store.isSelected(item.id)}
            <article class="smart-card" class:smart-card-off={!checked} in:listIn={{ index: i }}>
              <button
                type="button"
                role="checkbox"
                aria-checked={checked}
                class="check-box"
                class:check-on={checked}
                onclick={() => store.toggleSelected(item.id)}
              >
                <Icon name="check" size={12} stroke={2.4} class="check-icon" />
              </button>

              <span
                class="type-tile"
                style="color: {item.safety === 'safe' ? 'var(--color-ok)' : 'var(--color-warn)'}"
              >
                <Icon
                  name={item.path.includes('Movie') || item.path.includes('Video') ? 'film' : item.safety === 'safe' ? 'bolt' : 'file'}
                  size={16}
                />
              </span>

              <div class="smart-info">
                <p class="smart-name">{kindTitle(item)}</p>
                <p class="smart-meta num">{item.displayPath} · {formatSize(item.size)}</p>
                <p class="smart-reason">{reasonText(item)}</p>
                <p class="smart-impact">{impactText(item)}</p>
                <p class="smart-clean">
                  <span class="clean-method">{cleanupMethodText(item)}</span>
                  {#if cleanupCommandText(item)}
                    <code>{cleanupCommandText(item)}</code>
                  {/if}
                </p>
              </div>

              {#if item.safety === 'safe'}
                <span class="ai-badge">{t('sub.aiBadge')}</span>
              {:else}
                <span class="review-badge">{t('sub.reviewBadge')}</span>
              {/if}
            </article>
          {:else}
            <div class="panel-empty" in:fade={{ duration: 300 }}>
              <Icon name="check" size={24} style="color: var(--color-ok)" />
              <p>{t('sub.smartEmpty')}</p>
            </div>
          {/each}
        </div>
      </section>

      <!-- EXPLORER -->
      <section class="pager-panel" role="tabpanel">
        <div class="explorer">
          <div class="ex-crumb">
            <LocationPicker />
            {#each store.drillPath as seg, i (seg + i)}
              <Icon name="chevronRight" size={11} style="color: var(--color-faint)" />
              <button
                type="button"
                class="ex-crumb-item"
                class:ex-crumb-current={i === store.drillPath.length - 1}
                onclick={() => store.jumpCrumb(i)}
              >
                {seg}
              </button>
            {/each}
          </div>

          <div class="ex-body">
            <div class="ex-list" data-od-id="explorer-list">
              <div class="ex-cols">
                <span class="ex-col-icon"></span>
                <span class="ex-col-name">{t('sub.col.name')}</span>
                <span class="ex-col-note">{t('sub.col.note')}</span>
                <span class="ex-col-size">{t('sub.col.size')}</span>
                <span class="ex-col-action">{t('sub.col.action')}</span>
              </div>

              <ul class="ex-rows">
                {#if store.drillPath.length > 0}
                  <li class="ex-row ex-parent">
                    <span class="ex-row-icon" style="color: var(--color-faint)">
                      <Icon name="chevronUp" size={14} />
                    </span>
                    <button type="button" class="ex-parent-link" onclick={() => store.goUp()}>
                      ../ {t('sub.goUp')}
                    </button>
                    <span class="ex-note ex-note-faint">{t('sub.parentNote')}</span>
                    <span class="num ex-size"></span>
                    <span class="ex-action"></span>
                  </li>
                {/if}

                {#each store.listEntries as entry, i (entry.id)}
                  <li class="ex-row group" in:listIn={{ index: i }} oncontextmenu={(e) => openListContext(e, entry)}>
                    <span
                      class="ex-row-icon"
                      style="color: {entry.insightId && entry.risk === 'safe' ? 'var(--color-ok)' : entry.risk === 'review' ? 'var(--color-warn)' : 'var(--color-faint)'}"
                    >
                      {#if entry.insightId && entry.risk === 'keep'}
                        <Icon name="shield" size={14} />
                      {:else}
                        <Icon name={entry.isDir ? 'folder' : 'hardDrive'} size={14} />
                      {/if}
                    </span>

                    <button
                      type="button"
                      class="ex-name"
                      disabled={!entry.isDir}
                      onclick={() => store.drillInto(entry.name)}
                    >
                      <span class="ex-name-text">{entry.name}</span>
                      {#if entry.isDir}
                        <Icon
                          name="chevronRight"
                          size={12}
                          class="ml-auto shrink-0 opacity-0 transition-all group-hover:translate-x-0.5 group-hover:opacity-100"
                          style="color: var(--color-faint)"
                        />
                      {/if}
                    </button>

                    <span class="ex-note" class:ex-note-faint={!entry.insightId}>
                      {#if entry.insightId}
                        {reasonText(store.findings.find((f) => f.id === entry.insightId)!)}
                      {:else if entry.deletable === false}
                        {t('sub.noteProtected')}
                      {:else}
                        {t('sub.notePersonal')}
                      {/if}
                    </span>

                    <span class="num ex-size">
                      {#if entry.isDir && entry.status !== 'ok'}
                        <span class="ex-status ex-status-{entry.status}">
                          {t(`status.${entry.status}`)}
                        </span>
                      {/if}
                      {formatSize(entry.size)}
                    </span>

                    <span class="ex-action">
                      {#if entry.risk === 'keep'}
                        <span class="ex-protected">{t('sub.protected')}</span>
                      {:else if entry.deletable === false}
                        <span class="ex-protected">{t('sub.noPermission')}</span>
                      {:else if entry.insightId}
                        <button
                          type="button"
                          class="ex-sift-btn"
                          class:ex-sift-on={store.isSelected(entry.insightId)}
                          onclick={() => store.toggleSelected(entry.insightId ?? '')}
                        >
                          {#if store.isSelected(entry.insightId)}
                            <Icon name="undo" size={11} /> {t('sub.remove')}
                          {:else}
                            <Icon name="spark" size={11} /> {t('sub.clean')}
                          {/if}
                        </button>
                      {:else}
                        <button type="button" class="ex-sift-btn" onclick={() => store.addManualCandidate(entry)}>
                          <Icon name="trash" size={11} /> {t('sub.add')}
                        </button>
                      {/if}
                    </span>
                  </li>
                {:else}
                  <li class="ex-empty" in:fade={{ duration: 300 }}>
                    <Icon name="check" size={20} style="color: var(--color-ok)" />
                    {t('list.folderEmpty')}
                  </li>
                {/each}
              </ul>
            </div>
          </div>

          {#if listCtx}
            <div
              class="list-ctx"
              style="left: {listCtx.x}px; top: {listCtx.y}px"
              role="menu"
              in:scale={{ duration: 130, start: 0.96 }}
              out:scale={{ duration: 110, start: 0.96, opacity: 0 }}
            >
              {#if listCtx.node.risk === 'keep'}
                <button type="button" class="list-ctx-item" disabled>
                  <Icon name="shield" size={14} /> {t('list.protected')}
                </button>
              {:else}
                <button
                  type="button"
                  class="list-ctx-item"
                  class:list-ctx-danger={listCtx.inQueue}
                  class:list-ctx-off={listCtx.node.deletable === false}
                  disabled={listCtx.node.deletable === false}
                  role="menuitem"
                  onclick={confirmListContext}
                >
                  <Icon name={listCtx.inQueue ? 'undo' : 'trash'} size={14} />
                  {listCtx.inQueue ? t('ctx.remove') : t('ctx.add')}
                  {#if listCtx.node.deletable === false}
                    <span class="ctx-perm">{t('ctx.noPermission')}</span>
                  {/if}
                </button>
              {/if}
            </div>
          {/if}
        </div>
      </section>

      <!-- HISTORY -->
      <section class="pager-panel" role="tabpanel">
        <div class="hist-scroll">
          {#each store.history as rec, i (rec.id)}
            <article class="hist-card" in:listIn={{ index: i }}>
              <span class="hist-status">
                <Icon name="check" size={17} stroke={2.6} />
              </span>

              <div class="hist-info">
                <div class="hist-card-top">
                  <p class="hist-card-title">
                    {rec.automatic ? t('hist.autoTitle') : t('hist.manualTitle')}
                  </p>
                  {#if rec.automatic}
                    <span class="hist-auto-badge"><Icon name="bolt" size={10} /> {t('hist.autoBadge')}</span>
                  {/if}
                </div>
                <p class="hist-when">
                  {t('hist.when', [ago(rec.atMs), clockAt(rec.atMs), rec.items])}
                </p>

                <div class="hist-chips">
                  {#each rec.titles as title}
                    <span class="hist-chip">{title}</span>
                  {/each}
                </div>
              </div>

              <div class="hist-released">
                <span class="num hist-bytes">{formatSize(rec.bytes)}</span>
                <span class="hist-released-label">{t('hist.released')}</span>
              </div>
            </article>
          {:else}
            <div class="panel-empty" in:fade={{ duration: 300 }}>
              <Icon name="clock" size={24} style="color: var(--color-faint)" />
              <p>{t('hist.empty')}</p>
            </div>
          {/each}
        </div>
      </section>
    </div>
  </div>

  <footer class="sp-status">
    <span class="sp-status-l">
      {t('sub.statusOrder')}<span class="status-dot"></span>{t('sub.statusConfidence')}
    </span>
    {#if store.subTab === 'smart'}
      <span class="sp-status-r">
        <button type="button" class="status-link" onclick={selectAllSmart}>{t('sub.selectAll')}</button>
        <button type="button" class="status-link" onclick={deselectAllSmart}>{t('sub.deselectAll')}</button>
      </span>
    {/if}
  </footer>
</div>

<style>
  .subpage {
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .sp-head {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 18px clamp(14px, 2.6vw, 32px) 12px;
    flex: none;
  }
  .sp-back {
    width: 34px;
    height: 34px;
    padding: 0;
    border-radius: 10px;
    border: 1px solid var(--color-border);
  }
  .sp-total {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 32px;
    padding: 0 14px;
    border-radius: 10px;
    border: 1px solid var(--color-border);
    background: color-mix(in oklch, var(--color-surface) 80%, transparent);
    font-size: 12px;
    color: var(--color-faint);
    white-space: nowrap;
  }
  .sp-total-big {
    font-size: 13px;
    font-weight: 650;
    color: var(--color-fg);
  }
  .sp-confirm {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 34px;
    padding: 0 18px;
    border: none;
    border-radius: 10px;
    background: var(--color-cta-bg);
    color: var(--color-cta-fg);
    font-size: 13px;
    font-weight: 620;
    white-space: nowrap;
    cursor: default;
    box-shadow:
      0 1px 0 color-mix(in oklch, var(--color-sheen) 60%, transparent) inset,
      0 14px 30px -14px var(--color-shadow);
  }
  .sp-confirm:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .sp-tabbar {
    padding: 0 clamp(14px, 2.6vw, 32px);
    flex: none;
  }
  .segmented {
    position: relative;
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    padding: 3px;
    border-radius: 12px;
    background: color-mix(in oklch, var(--color-bg) 55%, transparent);
    box-shadow: inset 0 0 0 1px var(--color-border);
  }
  .segmented-thumb {
    position: absolute;
    top: 3px;
    bottom: 3px;
    left: 0;
    border-radius: 9px;
    background: color-mix(in oklch, var(--color-violet) 30%, var(--color-surface-2));
    box-shadow: inset 0 0 0 1px color-mix(in oklch, var(--color-violet) 50%, transparent);
    transition: transform 0.26s cubic-bezier(0.4, 0, 0.2, 1);
  }
  .seg-tab {
    position: relative;
    z-index: 1;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 7px;
    height: 30px;
    border: none;
    background: transparent;
    border-radius: 9px;
    font-size: 12.5px;
    font-weight: 550;
    color: var(--color-muted);
    cursor: default;
  }
  .seg-tab-on,
  .seg-tab-on:hover {
    color: oklch(0.92 0.04 300);
  }
  .sp-body {
    flex: 1;
    min-height: 0;
    margin: 14px clamp(14px, 2.6vw, 32px) 0;
    overflow: hidden;
    border-radius: 16px;
    border: 1px solid var(--color-border);
    background: color-mix(in oklch, var(--color-surface) 40%, transparent);
  }
  .pager {
    display: flex;
    height: 100%;
    width: 300%;
    transition: transform 0.32s cubic-bezier(0.22, 1, 0.36, 1);
  }
  .pager-panel {
    width: calc(100% / 3);
    min-width: 0;
    height: 100%;
    overflow: hidden;
  }
  .sp-status {
    display: flex;
    align-items: center;
    flex: none;
    padding: 10px 40px 12px;
  }
  .sp-status-l {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    font-family: var(--font-mono);
    font-size: 10.5px;
    letter-spacing: 0.08em;
    color: var(--color-faint);
  }
  .status-dot {
    width: 3px;
    height: 3px;
    border-radius: 50%;
    background: var(--color-border-strong);
  }
  .sp-status-r {
    margin-left: auto;
    display: inline-flex;
    gap: 14px;
  }
  .status-link {
    border: none;
    background: transparent;
    padding: 0;
    font-family: var(--font-mono);
    font-size: 10.5px;
    letter-spacing: 0.08em;
    color: var(--color-faint);
    cursor: default;
  }
  .status-link:hover {
    color: var(--color-fg);
  }
  .spin {
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .panel-empty {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    font-size: 12.5px;
    color: var(--color-faint);
  }
  .check-box {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    flex: none;
    border-radius: 6px;
    border: 1px solid var(--color-border-strong);
    background: transparent;
    color: transparent;
    cursor: default;
  }
  .check-box.check-on {
    background: var(--color-violet);
    border-color: var(--color-violet);
    color: var(--color-accent-contrast);
    box-shadow: 0 0 0 3px color-mix(in oklch, var(--color-violet) 22%, transparent);
  }
  .check-box :global(.check-icon) {
    transition: transform 0.18s cubic-bezier(0.2, 1.4, 0.4, 1);
    transform: scale(0.6);
  }
  .check-box.check-on :global(.check-icon) {
    transform: scale(1);
  }
  .smart-scroll {
    height: 100%;
    overflow-y: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .smart-card {
    display: flex;
    align-items: center;
    gap: 13px;
    padding: 13px 15px;
    border-radius: 13px;
    border: 1px solid var(--color-border);
    background: linear-gradient(
      180deg,
      color-mix(in oklch, var(--color-surface) 96%, var(--color-sheen) 1%),
      color-mix(in oklch, var(--color-surface) 98%, var(--color-shadow) 4%)
    );
  }
  .smart-card-off {
    opacity: 0.5;
  }
  .type-tile {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    flex: none;
    border-radius: 10px;
    background: color-mix(in oklch, currentColor 13%, transparent);
  }
  .smart-info {
    min-width: 0;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .smart-name {
    margin: 0;
    font-size: 13.5px;
    font-weight: 620;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .smart-meta {
    margin: 0;
    font-size: 10.5px;
    color: var(--color-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .smart-reason {
    margin: 0;
    font-size: 11px;
    color: var(--color-muted);
  }
  .smart-impact {
    margin: 2px 0 0;
    font-size: 11px;
    color: var(--color-faint);
  }
  .smart-clean {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 3px 0 0;
    font-size: 10.5px;
    color: var(--color-faint);
  }
  .smart-clean code {
    padding: 1px 6px;
    border-radius: 6px;
    background: color-mix(in srgb, var(--color-text) 8%, transparent);
    font-size: 10.5px;
  }
  .ai-badge,
  .review-badge {
    flex: none;
    display: inline-flex;
    align-items: center;
    height: 22px;
    padding: 0 10px;
    border-radius: 7px;
    font-family: var(--font-mono);
    font-size: 10px;
    font-weight: 650;
    letter-spacing: 0.06em;
  }
  .ai-badge {
    color: var(--color-ok);
    border: 1px solid color-mix(in oklch, var(--color-ok) 55%, transparent);
    background: color-mix(in oklch, var(--color-ok) 10%, transparent);
  }
  .review-badge {
    color: var(--color-warn);
    border: 1px solid color-mix(in oklch, var(--color-warn) 50%, transparent);
    background: color-mix(in oklch, var(--color-warn) 9%, transparent);
  }
  .explorer {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .ex-crumb {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 12px 16px 10px;
    flex: none;
  }
  .ex-crumb-item {
    border: none;
    background: transparent;
    padding: 3px 7px;
    border-radius: 7px;
    font-size: 12px;
    color: var(--color-muted);
    cursor: default;
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ex-crumb-item:hover {
    background: color-mix(in oklch, var(--color-surface-2) 70%, transparent);
    color: var(--color-fg);
  }
  .ex-crumb-current {
    color: var(--color-fg);
    font-weight: 600;
  }
  .ex-body {
    flex: 1;
    min-height: 0;
    padding: 0 16px 16px;
  }
  .ex-list {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .ex-cols {
    display: flex;
    align-items: center;
    gap: 8px;
    /* Match the rows' x-padding so every header sits over its column. */
    padding: 11px 8px 8px;
    flex: none;
    font-family: var(--font-mono);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.1em;
    color: var(--color-faint);
  }
  .ex-col-icon {
    width: 18px;
    flex: none;
  }
  .ex-col-name {
    flex: 0 1 150px;
    min-width: 56px;
    white-space: nowrap;
    overflow: hidden;
  }
  .ex-col-size {
    width: 104px;
    flex: none;
    text-align: right;
    white-space: nowrap;
  }
  .ex-col-note {
    flex: 1 1 0;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
  }
  .ex-col-action {
    /* Wide enough for "No permission" on one line. */
    width: 84px;
    flex: none;
    text-align: right;
    white-space: nowrap;
  }
  .ex-rows {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    margin: 0;
    /* No x-padding here: each row carries its own 8px, matching the header. */
    padding: 2px 0 10px;
    list-style: none;
  }
  .ex-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 7px 8px;
    border-radius: 9px;
  }
  .ex-row-icon {
    width: 18px;
    flex: none;
    display: inline-flex;
    justify-content: center;
  }
  .ex-name {
    /* Shrink the name (it ellipsizes) before the fixed size/action columns
       can overlap. */
    flex: 0 1 150px;
    min-width: 56px;
    display: flex;
    align-items: center;
    gap: 4px;
    border: none;
    background: transparent;
    padding: 0;
    text-align: left;
    cursor: default;
  }
  .ex-name-text {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12.5px;
    color: var(--color-fg);
  }
  .ex-size {
    /* Room for a status badge + the size on one line, so neither spills into
       the action button. */
    width: 104px;
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: flex-end;
    gap: 6px;
    font-size: 11.5px;
    color: var(--color-muted);
  }
  .ex-status {
    padding: 1px 6px;
    border-radius: 5px;
    font-size: 9.5px;
    font-weight: 650;
    letter-spacing: 0.02em;
  }
  .ex-status-estimated {
    color: var(--color-warn);
    background: color-mix(in oklch, var(--color-warn) 12%, transparent);
  }
  .ex-status-denied {
    color: var(--color-danger);
    background: color-mix(in oklch, var(--color-danger) 12%, transparent);
  }
  .ex-status-awaiting {
    color: var(--color-violet);
    background: color-mix(in oklch, var(--color-violet) 13%, transparent);
  }
  .ex-action {
    width: 84px;
    flex: none;
    display: flex;
    justify-content: flex-end;
  }
  .ex-protected {
    font-size: 10.5px;
    color: var(--color-faint);
    white-space: nowrap;
  }
  .ex-sift-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 24px;
    padding: 0 9px;
    border-radius: 7px;
    border: 1px solid var(--color-border-strong);
    background: transparent;
    color: var(--color-muted);
    font-size: 10.5px;
    font-weight: 550;
    cursor: default;
  }
  .ex-sift-btn:hover {
    color: var(--color-fg);
    border-color: var(--color-faint);
  }
  .ex-sift-btn.ex-sift-on {
    color: var(--color-accent-hi);
    border-color: color-mix(in oklch, var(--color-accent) 55%, transparent);
    background: color-mix(in oklch, var(--color-accent) 14%, transparent);
  }
  .ex-parent-link {
    width: 150px;
    flex: none;
    border: none;
    background: transparent;
    padding: 0;
    font-size: 12px;
    color: var(--color-faint);
    cursor: default;
    text-align: left;
  }
  .ex-note {
    flex: 1;
    min-width: 0;
    font-size: 11.5px;
    color: var(--color-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ex-note-faint {
    color: var(--color-faint);
  }
  .ex-empty {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 30px 10px;
    font-size: 12px;
    color: var(--color-faint);
  }
  .list-ctx {
    position: absolute;
    z-index: 40;
    width: 196px;
    padding: 5px;
    border-radius: 12px;
    background: color-mix(in oklch, var(--color-surface-2) 92%, var(--color-bg));
    border: 1px solid var(--color-border-strong);
    box-shadow:
      0 2px 8px -2px oklch(0% 0 0 / 0.5),
      0 18px 44px -12px oklch(0% 0 0 / 0.6);
    transform-origin: top left;
  }
  .list-ctx-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 9px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--color-fg);
    font-size: 12.5px;
    text-align: left;
    cursor: default;
  }
  .list-ctx-item:hover:not(:disabled) {
    background: color-mix(in oklch, var(--color-accent) 16%, transparent);
  }
  .list-ctx-danger:hover:not(:disabled) {
    background: color-mix(in oklch, var(--color-danger) 18%, transparent);
  }
  .list-ctx-item:disabled {
    color: var(--color-muted);
    cursor: default;
  }
  .list-ctx-off {
    color: var(--color-faint);
  }
  .ctx-perm {
    margin-left: auto;
    font-size: 10px;
    color: var(--color-faint);
  }
  .hist-scroll {
    height: 100%;
    overflow-y: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .hist-card {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 15px 17px;
    border-radius: 13px;
    border: 1px solid var(--color-border);
    background: linear-gradient(
      180deg,
      color-mix(in oklch, var(--color-surface) 96%, var(--color-sheen) 1%),
      color-mix(in oklch, var(--color-surface) 98%, var(--color-shadow) 4%)
    );
  }
  .hist-status {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 40px;
    height: 40px;
    flex: none;
    border-radius: 11px;
    color: var(--color-ok);
    background: color-mix(in oklch, var(--color-ok) 15%, transparent);
  }
  .hist-info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .hist-card-top {
    display: flex;
    align-items: center;
    gap: 9px;
  }
  .hist-card-title {
    margin: 0;
    font-size: 14px;
    font-weight: 630;
  }
  .hist-auto-badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 19px;
    padding: 0 7px;
    border-radius: 6px;
    font-size: 10px;
    font-weight: 600;
    color: var(--color-accent-hi);
    background: color-mix(in oklch, var(--color-accent) 14%, transparent);
  }
  .hist-when {
    margin: 0;
    font-size: 11.5px;
    color: var(--color-faint);
  }
  .hist-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 2px;
  }
  .hist-chip {
    font-size: 10.5px;
    padding: 3px 9px;
    border-radius: 7px;
    background: color-mix(in oklch, var(--color-surface-2) 80%, transparent);
    color: var(--color-muted);
  }
  .hist-released {
    flex: none;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 2px;
  }
  .hist-bytes {
    font-size: 22px;
    font-weight: 650;
    letter-spacing: -0.02em;
    color: var(--color-ok);
  }
  .hist-released-label {
    font-family: var(--font-mono);
    font-size: 9.5px;
    letter-spacing: 0.14em;
    color: color-mix(in oklch, var(--color-ok) 80%, var(--color-sheen));
  }
</style>

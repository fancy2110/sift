<script lang="ts">
  import Icon from '../lib/components/Icon.svelte';
  import LocationPicker from '../lib/components/LocationPicker.svelte';
  import { store } from '../lib/store.svelte';
  import { formatSize } from '../lib/format';
  import type { FileNode, Risk, SubTab } from '../lib/types';
  import { fly, fade, scale } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';

  const riskColor: Record<Risk, string> = {
    safe: 'var(--color-ok)',
    review: 'var(--color-warn)',
    keep: 'var(--color-faint)'
  };

  // ---- smart panel data ---------------------------------------------------
  const smartItems = $derived(store.visible.filter((i) => i.risk !== 'keep'));

  function selectAllSmart() {
    store.selectedIds = new Set(smartItems.map((i) => i.id));
  }
  function deselectAllSmart() {
    store.selectedIds = new Set();
  }

  // ---- shared clean action -----------------------------------------------
  async function confirmSift() {
    await store.clean(false);
  }

  // ---- explorer row right-click menu --------------------------------------
  let listCtx = $state<{ x: number; y: number; node: FileNode; inQueue: boolean } | null>(
    null
  );

  function openListContext(e: MouseEvent, node: FileNode) {
    e.preventDefault();
    const host = (e.currentTarget as HTMLElement).closest<HTMLElement>(
      '[data-od-id="explorer-list"]'
    );
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

  $effect(() => {
    void store.drillPath.length;
    void store.currentLocId;
    void store.treeVersion;
    listCtx = null;
  });

  // ---- history helpers ----------------------------------------------------
  function ago(ts: number): string {
    const d = Math.round((Date.now() - ts) / 86400000);
    if (d <= 0) return '今天';
    if (d === 1) return '昨天';
    return `${d} 天前`;
  }
  function clockAt(ts: number): string {
    return new Date(ts).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
  }

  // ---- shared transitions -------------------------------------------------
  function listIn(_el: Element, { index, base = 0 }: { index: number; base?: number }) {
    return {
      duration: 300,
      delay: base + Math.min(index, 8) * 30,
      easing: cubicOut,
      css: (t: number) =>
        `opacity:${t};transform:translateY(${(12 * (1 - t)).toFixed(1)}px) scale(${(0.99 + 0.01 * t).toFixed(4)})`
    };
  }

  const tabIndex: Record<SubTab, number> = { smart: 0, explorer: 1, history: 2 };
  const pagerOffset = $derived(tabIndex[store.subTab] * (100 / 3));

  const TABS: [SubTab, string, string][] = [
    ['smart', 'spark', '智能'],
    ['explorer', 'layers', '浏览'],
    ['history', 'clock', '历史']
  ];
</script>

<div class="subpage" data-od-id="sub-page">
  <!-- stable header -->
  <header class="sp-head">
    <button
      type="button"
      class="btn btn-quiet sp-back"
      onclick={() => store.goHome()}
      data-od-id="sub-back"
      aria-label="返回"
    >
      <Icon name="chevronLeft" size={16} />
    </button>

    <span class="sp-total" data-od-id="sub-total">
      <span class="sp-total-label">待删除</span>
      <span class="num sp-total-big">{formatSize(store.selectedBytes)}</span>
    </span>

    <button
      type="button"
      class="sp-confirm"
      disabled={store.selectedIds.size === 0 || store.cleaning}
      onclick={confirmSift}
      data-od-id="sub-confirm"
    >
      {#if store.cleaning}
        <span class="spin" style="display: inline-flex"><Icon name="refresh" size={14} /></span>
        清理中
      {:else}
        <Icon name="trash" size={14} />
        确认清除
      {/if}
    </button>
  </header>

  <!-- segmented tabs -->
  <div class="sp-tabbar">
    <div class="segmented" role="tablist" aria-label="子页面导航">
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
          data-od-id="seg-{tabId}"
        >
          <Icon name={icon} size={13.5} />
          {label}
        </button>
      {/each}
    </div>
  </div>

  <!-- pager -->
  <div class="sp-body">
    <div
      class="pager"
      style="transform: translateX(-{pagerOffset}%)"
    >
      <!-- SMART -->
      <section class="pager-panel" role="tabpanel" data-od-id="panel-smart">
        <div class="smart-scroll" data-od-id="smart-scroll">
          {#each smartItems as item, i (item.id)}
            {@const checked = store.isSelected(item.id)}
            <article
              class="smart-card"
              class:smart-card-off={!checked}
              in:listIn={{ index: i }}
            >
              <button
                type="button"
                role="checkbox"
                aria-checked={checked}
                class="check-box"
                class:check-on={checked}
                onclick={() => store.toggleSelected(item.id)}
                data-od-id="smart-check-{item.id}"
              >
                <Icon name="check" size={12} stroke={2.4} class="check-icon" />
              </button>

              <span class="type-tile" style="color: {riskColor[item.risk]}">
                <Icon
                  name={item.path.includes('Movie') || item.path.includes('Video') ? 'film' : item.risk === 'safe' ? 'bolt' : 'file'}
                  size={16}
                />
              </span>

              <div class="smart-info">
                <p class="smart-name">{item.title}</p>
                <p class="smart-meta num">{item.path} · {formatSize(item.size)}</p>
              </div>

              {#if item.risk === 'safe'}
                <span class="ai-badge">AI 推荐</span>
              {:else}
                <span class="review-badge">待确认</span>
              {/if}
            </article>
          {:else}
            <div class="panel-empty" in:fade={{ duration: 300 }}>
              <Icon name="check" size={24} style="color: var(--color-ok)" />
              <p>没有待处理的内容</p>
            </div>
          {/each}
        </div>
      </section>

      <!-- EXPLORER -->
      <section class="pager-panel" role="tabpanel" data-od-id="panel-explorer">
        <div class="explorer" data-od-id="explorer-panel">
          <!-- breadcrumb: disk picker is the root crumb -->
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
            <!-- list -->
            <div class="ex-list" data-od-id="explorer-list">
              <div class="ex-cols">
                <span>名称</span>
                <span class="ex-col-note">说明</span>
                <span class="ex-col-size">大小</span>
                <span class="ex-col-action">操作</span>
              </div>

              <ul class="ex-rows">
                {#if store.drillPath.length > 0}
                  <li class="ex-row ex-parent">
                    <span class="ex-row-icon" style="color: var(--color-faint)">
                      <Icon name="chevronUp" size={14} />
                    </span>
                    <button type="button" class="ex-parent-link" onclick={() => store.goUp()}>
                      ../ 返回上级目录
                    </button>
                    <span class="ex-note ex-note-faint">上一级目录</span>
                    <span class="num ex-size"></span>
                    <span class="ex-action"></span>
                  </li>
                {/if}

                {#each store.listEntries as entry, i (entry.name)}
                  <li class="ex-row group" in:listIn={{ index: i }} oncontextmenu={(e) => openListContext(e, entry)}>
                    <span
                      class="ex-row-icon"
                      style="color: {entry.insightId && entry.risk ? riskColor[entry.risk] : 'var(--color-faint)'}"
                    >
                      {#if entry.insightId && entry.risk === 'keep'}
                        <Icon name="shield" size={14} />
                      {:else}
                        <Icon name={entry.children ? 'folder' : 'hardDrive'} size={14} />
                      {/if}
                    </span>

                    <button
                      type="button"
                      class="ex-name"
                      disabled={!entry.children?.length}
                      onclick={() => entry.children && store.drillInto(entry.name)}
                    >
                      <span class="ex-name-text">{entry.name}</span>
                      {#if entry.children}
                        <Icon
                          name="chevronRight"
                          size={12}
                          class="ml-auto shrink-0 opacity-0 transition-all group-hover:translate-x-0.5 group-hover:opacity-100"
                          style="color: var(--color-faint)"
                        />
                      {/if}
                    </button>

                    <span class="ex-note" class:ex-note-faint={!entry.note}>
                      {entry.note ??
                        (entry.risk === 'keep'
                          ? 'AI 已主动保护'
                          : entry.deletable === false
                            ? '系统目录，受保护'
                            : '个人文件与数据')}
                    </span>

                    <span class="num ex-size">{formatSize(entry.size)}</span>

                    <span class="ex-action">
                      {#if entry.risk === 'keep'}
                        <span class="ex-protected">已保护</span>
                      {:else if entry.deletable === false}
                        <span class="ex-protected">无权限</span>
                      {:else if entry.insightId}
                        <button
                          type="button"
                          class="ex-sift-btn"
                          class:ex-sift-on={store.isSelected(entry.insightId)}
                          onclick={() => store.toggleSelected(entry.insightId!)}
                        >
                          {#if store.isSelected(entry.insightId)}
                            <Icon name="undo" size={11} /> 移出
                          {:else}
                            <Icon name="spark" size={11} /> 清理
                          {/if}
                        </button>
                      {:else}
                        <button
                          type="button"
                          class="ex-sift-btn"
                          onclick={() => store.addManualCandidate(entry)}
                        >
                          <Icon name="trash" size={11} /> 加入
                        </button>
                      {/if}
                    </span>
                  </li>
                {:else}
                  <li class="ex-empty" in:fade={{ duration: 300 }}>
                    <Icon name="check" size={20} style="color: var(--color-ok)" />
                    此文件夹没有可整理的内容
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
                  <Icon name="shield" size={14} /> AI 已保护，不可删除
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
                  {listCtx.inQueue ? '从删除队列移除' : '添加到删除队列'}
                  {#if listCtx.node.deletable === false}
                    <span class="ctx-perm">无权限</span>
                  {/if}
                </button>
              {/if}
            </div>
          {/if}
        </div>
      </section>

      <!-- HISTORY -->
      <section class="pager-panel" role="tabpanel" data-od-id="panel-history">
        <div class="hist-scroll" data-od-id="hist-scroll">
          {#each store.history as rec, i (rec.id)}
            <article class="hist-card" in:listIn={{ index: i }}>
              <span class="hist-status">
                <Icon name="check" size={17} stroke={2.6} />
              </span>

              <div class="hist-info">
                <div class="hist-card-top">
                  <p class="hist-card-title">
                    {rec.automatic ? '定时深度清理' : '手动整理'}
                  </p>
                  {#if rec.automatic}
                    <span class="hist-auto-badge"><Icon name="bolt" size={10} /> 自动</span>
                  {/if}
                </div>
                <p class="hist-when">完成于 {ago(rec.at)} · {clockAt(rec.at)} · {rec.items} 项</p>

                <div class="hist-chips">
                  {#each rec.titles as t}
                    <span class="hist-chip">{t}</span>
                  {/each}
                </div>
              </div>

              <div class="hist-released">
                <span class="num hist-bytes">{formatSize(rec.bytes)}</span>
                <span class="hist-released-label">已释放</span>
              </div>
            </article>
          {:else}
            <div class="panel-empty" in:fade={{ duration: 300 }}>
              <Icon name="clock" size={24} style="color: var(--color-faint)" />
              <p>还没有清理记录</p>
            </div>
          {/each}
        </div>
      </section>
    </div>
  </div>

  <!-- status bar -->
  <footer class="sp-status">
    <span class="sp-status-l">
      按大小降序排列<span class="status-dot"></span>AI 引擎置信度 99.8%
    </span>
    {#if store.subTab === 'smart'}
      <span class="sp-status-r">
        <button type="button" class="status-link" onclick={selectAllSmart}>全选</button>
        <button type="button" class="status-link" onclick={deselectAllSmart}>取消全选</button>
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

  /* header */
  .sp-head {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 20px 40px 14px;
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
    background: #f4f4f6;
    color: #141418;
    font-size: 13px;
    font-weight: 620;
    cursor: default;
    box-shadow:
      0 1px 0 rgba(255, 255, 255, 0.6) inset,
      0 14px 30px -14px rgba(0, 0, 0, 0.8);
    transition: transform 0.15s ease, box-shadow 0.15s ease;
  }
  .sp-confirm:hover:not(:disabled) {
    transform: translateY(-1px);
  }
  .sp-confirm:active {
    transform: translateY(0);
  }
  .sp-confirm:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .sp-confirm:focus-visible {
    outline: 2px solid color-mix(in oklch, var(--color-accent) 70%, transparent);
    outline-offset: 3px;
  }

  /* segmented tabs */
  .sp-tabbar {
    padding: 0 40px;
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
    box-shadow:
      inset 0 0 0 1px color-mix(in oklch, var(--color-violet) 50%, transparent),
      0 1px 4px -1px oklch(0% 0 0 / 0.5);
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
    transition: color 0.18s ease;
  }
  .seg-tab:hover {
    color: var(--color-fg);
  }
  .seg-tab-on,
  .seg-tab-on:hover {
    color: oklch(0.92 0.04 300);
  }
  .seg-tab:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--color-bg), 0 0 0 4px var(--color-violet);
  }

  /* pager */
  .sp-body {
    flex: 1;
    min-height: 0;
    margin: 14px 40px 0;
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

  /* status bar */
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
    transition: color 0.14s ease;
  }
  .status-link:hover {
    color: var(--color-fg);
  }
  .status-link:focus-visible {
    outline: 2px solid color-mix(in oklch, var(--color-accent) 60%, transparent);
    outline-offset: 2px;
  }

  .spin {
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  /* ---- shared empty state ---- */
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

  /* ---- checkbox ---- */
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
    transition: background 0.16s ease, border-color 0.16s ease, box-shadow 0.2s ease,
      transform 0.12s ease;
  }
  .check-box:hover {
    border-color: var(--color-violet);
  }
  .check-box:active {
    transform: scale(0.88);
  }
  .check-box:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--color-surface), 0 0 0 4px var(--color-violet);
  }
  .check-box :global(.check-icon) {
    transition: transform 0.18s cubic-bezier(0.2, 1.4, 0.4, 1);
    transform: scale(0.6);
  }
  .check-box.check-on {
    background: var(--color-violet);
    border-color: var(--color-violet);
    color: #fff;
    box-shadow: 0 0 0 3px color-mix(in oklch, var(--color-violet) 22%, transparent);
  }
  .check-box.check-on :global(.check-icon) {
    transform: scale(1);
  }

  /* ================= SMART ================= */
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
      color-mix(in oklch, var(--color-surface) 96%, white 1%),
      color-mix(in oklch, var(--color-surface) 98%, black 4%)
    );
    transition: opacity 0.2s ease, border-color 0.18s ease, transform 0.16s ease;
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

  /* ================= EXPLORER ================= */
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
    transition: background 0.14s ease, color 0.14s ease;
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
    display: flex;
    gap: 0;
    padding: 0 16px 16px;
  }
  .ex-list {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .ex-cols {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 11px 16px 8px;
    flex: none;
    font-family: var(--font-mono);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.1em;
    color: var(--color-faint);
  }
  .ex-col-size {
    width: 62px;
    text-align: right;
  }
  .ex-col-note {
    flex: 1;
    min-width: 0;
  }
  .ex-col-action {
    width: 60px;
    text-align: right;
  }
  .ex-rows {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    margin: 0;
    padding: 2px 8px 10px;
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
    width: 150px;
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    border: none;
    background: transparent;
    padding: 0;
    text-align: left;
    cursor: default;
  }
  .ex-name:disabled {
    cursor: default;
  }
  .ex-name:not(:disabled) {
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
    width: 62px;
    flex: none;
    text-align: right;
    font-size: 11.5px;
    color: var(--color-muted);
  }
  .ex-action {
    width: 60px;
    flex: none;
    display: flex;
    justify-content: flex-end;
  }
  .ex-protected {
    font-size: 10.5px;
    color: var(--color-faint);
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
    transition: background 0.14s ease, color 0.14s ease, border-color 0.14s ease;
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
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
  .ex-parent-link:hover {
    color: var(--color-fg);
  }
  .ex-empty {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 30px 10px;
    font-size: 12px;
    color: var(--color-faint);
  }

  /* row right-click menu */
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

  /* ================= HISTORY ================= */
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
      color-mix(in oklch, var(--color-surface) 96%, white 1%),
      color-mix(in oklch, var(--color-surface) 98%, black 4%)
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
    color: color-mix(in oklch, var(--color-ok) 80%, white);
  }

  @media (max-width: 900px) {
    .ex-body {
      flex-direction: column;
    }
    .ex-list {
      width: 100%;
      border-left: none;
      border-top: 1px solid var(--color-border);
    }
  }
</style>

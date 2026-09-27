<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { formatSize } from '../lib/format';
  import Icon from '../lib/components/Icon.svelte';
  import { fly, fade } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { volumes } from '../lib/data';

  let reviewBytes = $derived(
    store.insights
      .filter((i) => i.risk === 'review' && !store.resolvedIds.has(i.id))
      .reduce((s, i) => s + i.size, 0)
  );
  let keepBytes = $derived(
    store.insights
      .filter((i) => i.risk === 'keep' && !store.resolvedIds.has(i.id))
      .reduce((s, i) => s + i.size, 0)
  );

  function liveVolume(id: string) {
    return store.diskTrees[id];
  }
  function ago(ts: number): string {
    const d = Math.round((Date.now() - ts) / 86400000);
    if (d <= 0) return '今天';
    if (d === 1) return '昨天';
    return `${d} 天前`;
  }
</script>

<div class="dash" data-od-id="dashboard-view">
  <header class="d-head">
    <button class="btn btn-quiet back" onclick={() => store.goHome()} data-od-id="dash-back">
      <Icon name="chevronLeft" size={15} />
    </button>
    <div>
      <h1 class="d-title">仪表盘</h1>
      <p class="d-sub">磁盘健康、AI 分析与例行任务的总览</p>
    </div>
    <span class="chip ml-auto">
      <span class="engine-dot"></span> 引擎运行中
    </span>
  </header>

  <div class="d-scroll">
    <!-- KPI strip -->
    <section class="kpis">
      <div class="kpi" in:fly={{ y: 14, duration: 440, easing: cubicOut }}>
        <span class="kpi-label">可释放总量</span>
        <span class="kpi-big num">{formatSize(store.totalReclaimable)}</span>
        <span class="kpi-foot" style="color: var(--color-ok)">安全 {formatSize(store.safeReclaimable)}</span>
      </div>
      <div class="kpi" in:fly={{ y: 14, duration: 440, delay: 50, easing: cubicOut }}>
        <span class="kpi-label">待你确认</span>
        <span class="kpi-big num">{formatSize(reviewBytes)}</span>
        <span class="kpi-foot" style="color: var(--color-warn)">{store.pendingReviewCount} 个项目</span>
      </div>
      <div class="kpi" in:fly={{ y: 14, duration: 440, delay: 100, easing: cubicOut }}>
        <span class="kpi-label">AI 已保护</span>
        <span class="kpi-big num">{formatSize(keepBytes)}</span>
        <span class="kpi-foot">近期仍在使用</span>
      </div>
      <div class="kpi" in:fly={{ y: 14, duration: 440, delay: 150, easing: cubicOut }}>
        <span class="kpi-label">累计释放</span>
        <span class="kpi-big num">{formatSize(store.allTimeReclaimed)}</span>
        <span class="kpi-foot">{store.history.length} 次整理</span>
      </div>
    </section>

    <div class="d-grid">
      <!-- left: volumes -->
      <section class="panel" in:fly={{ y: 18, duration: 480, delay: 120, easing: cubicOut }}>
        <div class="panel-head">
          <h2 class="section-title">磁盘</h2>
          <button class="btn btn-quiet btn-sm" onclick={() => store.goSub('explorer')}>浏览</button>
        </div>

        <div class="vol-list">
          {#each volumes as vol, i}
            {@const tree = liveVolume(vol.id)}
            {@const used = tree?.size ?? vol.used}
            {@const pct = Math.min(Math.round((used / vol.capacity) * 100), 100)}
            {@const free = Math.max(vol.capacity - used, 0)}
            {@const tone = pct > 88 ? 'var(--color-danger)' : pct > 72 ? 'var(--color-warn)' : 'var(--color-ok)'}
            <div class="vol" style="animation-delay: {i * 70}ms">
              <div class="vol-top">
                <span class="vol-name">
                  <Icon name={vol.external ? 'externalDrive' : 'drive'} size={15} style="color: {tone}" />
                  {vol.name}
                </span>
                <span class="num vol-pct" style="color: {tone}">{pct}%</span>
              </div>
              <div class="vol-track">
                <div class="vol-fill" style="width: {pct}%; background: {tone}"></div>
              </div>
              <div class="vol-meta">
                <span class="num">{formatSize(free)} 可用</span>
                <span class="num">{formatSize(vol.capacity)}</span>
              </div>
            </div>
          {/each}
        </div>

        <!-- verdict composition -->
        <div class="comp">
          <h3 class="comp-title">AI 判定构成</h3>
          <div class="comp-bar">
            {#if store.safeReclaimable > 0}
              <span class="comp-seg" style="flex-grow: {store.safeReclaimable}; background: var(--color-ok)" title="安全清理"></span>
            {/if}
            {#if reviewBytes > 0}
              <span class="comp-seg" style="flex-grow: {reviewBytes}; background: var(--color-warn)" title="建议确认"></span>
            {/if}
            {#if keepBytes > 0}
              <span class="comp-seg" style="flex-grow: {keepBytes}; background: var(--color-surface-3)" title="已保护"></span>
            {/if}
          </div>
          <div class="comp-legend">
            <span><i style="background: var(--color-ok)"></i>安全 {formatSize(store.safeReclaimable)}</span>
            <span><i style="background: var(--color-warn)"></i>确认 {formatSize(reviewBytes)}</span>
            <span><i style="background: var(--color-surface-3)"></i>保护 {formatSize(keepBytes)}</span>
          </div>
        </div>
      </section>

      <!-- right: routines + recent -->
      <section class="panel" in:fly={{ y: 18, duration: 480, delay: 180, easing: cubicOut }}>
        <div class="panel-head">
          <h2 class="section-title">例行任务</h2>
          <span class="chip">{store.routines.length} 项</span>
        </div>
        <div class="rt-list">
          {#each store.routines as rt}
            <div class="rt-row">
              <div class="rt-info">
                <span class="rt-name">{rt.title}</span>
                <span class="rt-cad">{rt.cadence} · 平均 {formatSize(rt.avgSize)}</span>
              </div>
              <span class="rt-mode" class:auto={rt.autoMode === 'auto'}>
                {rt.autoMode === 'auto' ? '自动' : '确认'}
              </span>
            </div>
          {:else}
            <p class="empty">重复的整理决策会在这里自动沉淀</p>
          {/each}
        </div>

        <div class="panel-head recent-head">
          <h2 class="section-title">最近整理</h2>
          <button class="btn btn-quiet btn-sm" onclick={() => store.goSub('history')}>全部</button>
        </div>
        <div class="recent">
          {#each store.history.slice(0, 4) as rec}
            <div class="rec-row">
              <span class="rec-dot" class:auto={rec.automatic}></span>
              <span class="rec-titles">{rec.titles[0] ?? '整理'}</span>
              <span class="num rec-bytes">{formatSize(rec.bytes)}</span>
              <span class="rec-when">{ago(rec.at)}</span>
            </div>
          {:else}
            <p class="empty">还没有清理记录</p>
          {/each}
        </div>
      </section>
    </div>
  </div>
</div>

<style>
  .dash {
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .d-head {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 22px 44px 16px;
    flex: none;
  }
  .back {
    width: 32px;
    padding: 0;
    border-radius: 10px;
  }
  .d-title {
    margin: 0;
    font-size: 20px;
    font-weight: 650;
    letter-spacing: -0.02em;
  }
  .d-sub {
    margin: 2px 0 0;
    font-size: 12.5px;
    color: var(--color-faint);
  }
  .engine-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--color-ok);
    box-shadow: 0 0 8px var(--color-ok);
  }

  .d-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 4px 44px 30px;
  }

  .kpis {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 14px;
  }
  .kpi {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 18px 20px;
    border-radius: var(--radius-card);
    border: 1px solid var(--color-border);
    background: linear-gradient(
      180deg,
      color-mix(in oklch, var(--color-surface) 100%, white 1.2%),
      var(--color-surface)
    );
    box-shadow: 0 18px 40px -28px black;
  }
  .kpi-label {
    font-family: var(--font-mono);
    font-size: 10.5px;
    letter-spacing: 0.12em;
    color: var(--color-faint);
  }
  .kpi-big {
    font-size: 26px;
    font-weight: 630;
    letter-spacing: -0.02em;
    line-height: 1.1;
  }
  .kpi-foot {
    font-size: 11.5px;
    color: var(--color-muted);
  }

  .d-grid {
    display: grid;
    grid-template-columns: 1.55fr 1fr;
    gap: 14px;
    margin-top: 14px;
  }
  .panel {
    padding: 19px 21px 21px;
    border-radius: var(--radius-card);
    border: 1px solid var(--color-border);
    background: linear-gradient(
      180deg,
      color-mix(in oklch, var(--color-surface) 100%, white 1%),
      var(--color-surface)
    );
    box-shadow: 0 18px 40px -28px black;
  }
  .panel-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 16px;
  }

  .vol-list {
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .vol-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }
  .vol-name {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 13.5px;
    font-weight: 600;
  }
  .vol-pct {
    font-size: 13px;
    font-weight: 650;
  }
  .vol-track {
    height: 8px;
    border-radius: 999px;
    background: var(--color-surface-3);
    overflow: hidden;
  }
  .vol-fill {
    height: 100%;
    border-radius: 999px;
    transition: width 0.6s cubic-bezier(0.22, 1, 0.36, 1);
  }
  .vol-meta {
    display: flex;
    justify-content: space-between;
    margin-top: 7px;
    font-size: 11.5px;
    color: var(--color-faint);
  }

  .comp {
    margin-top: 24px;
    padding-top: 18px;
    border-top: 1px solid color-mix(in oklch, var(--color-border) 70%, transparent);
  }
  .comp-title {
    margin: 0 0 10px;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--color-muted);
  }
  .comp-bar {
    display: flex;
    height: 10px;
    border-radius: 999px;
    overflow: hidden;
    background: var(--color-surface-3);
  }
  .comp-seg {
    height: 100%;
  }
  .comp-legend {
    display: flex;
    gap: 18px;
    margin-top: 11px;
    font-size: 11.5px;
    color: var(--color-muted);
  }
  .comp-legend i {
    display: inline-block;
    width: 7px;
    height: 7px;
    border-radius: 2px;
    margin-right: 6px;
  }

  .rt-list {
    display: flex;
    flex-direction: column;
  }
  .rt-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 11px 0;
    border-bottom: 1px solid color-mix(in oklch, var(--color-border) 60%, transparent);
  }
  .rt-row:last-child {
    border-bottom: none;
  }
  .rt-info {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .rt-name {
    font-size: 13px;
    font-weight: 600;
  }
  .rt-cad {
    font-size: 11px;
    color: var(--color-faint);
  }
  .rt-mode {
    font-family: var(--font-mono);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.04em;
    padding: 3px 9px;
    border-radius: 6px;
    background: var(--color-surface-2);
    color: var(--color-muted);
  }
  .rt-mode.auto {
    background: color-mix(in oklch, var(--color-accent) 16%, transparent);
    color: var(--color-accent-hi);
  }

  .recent-head {
    margin: 20px 0 10px;
    padding-top: 16px;
    border-top: 1px solid color-mix(in oklch, var(--color-border) 70%, transparent);
  }
  .recent {
    display: flex;
    flex-direction: column;
  }
  .rec-row {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 9px 0;
    font-size: 12px;
  }
  .rec-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex: none;
    background: var(--color-faint);
  }
  .rec-dot.auto {
    background: var(--color-accent-hi);
  }
  .rec-titles {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--color-fg);
  }
  .rec-bytes {
    color: var(--color-muted);
  }
  .rec-when {
    width: 46px;
    text-align: right;
    font-size: 11px;
    color: var(--color-faint);
  }

  .empty {
    margin: 8px 0;
    font-size: 12px;
    color: var(--color-faint);
  }

  @media (max-width: 980px) {
    .kpis {
      grid-template-columns: repeat(2, 1fr);
    }
    .d-grid {
      grid-template-columns: 1fr;
    }
  }
</style>

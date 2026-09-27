<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { formatSize } from '../lib/format';
  import Icon from '../lib/components/Icon.svelte';
  import { fly, fade } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { t } from '../lib/i18n.svelte';

  function ago(ts: number): string {
    const d = Math.round((Date.now() - ts) / 86400000);
    if (d <= 0) return t('time.today');
    if (d === 1) return t('time.yesterday');
    return t('time.daysAgo', [d]);
  }
</script>

<div class="dash">
  <header class="d-head">
    <button class="btn btn-quiet back" onclick={() => store.goHome()}>
      <Icon name="chevronLeft" size={15} />
    </button>
    <div>
      <h1 class="d-title">{t('dash.title')}</h1>
      <p class="d-sub">{t('dash.sub')}</p>
    </div>
    <span class="chip ml-auto">
      <span class="engine-dot"></span> {t('dash.engineRunning')}
    </span>
  </header>

  <div class="d-scroll">
    <section class="kpis">
      <div class="kpi" in:fly={{ y: 14, duration: 440, easing: cubicOut }}>
        <span class="kpi-label">{t('dash.kpi.reclaimable')}</span>
        <span class="kpi-big num">{formatSize(store.totalReclaimable)}</span>
        <span class="kpi-foot" style="color: var(--color-ok)">
          {t('dash.kpi.safe', [formatSize(store.safeReclaimable)])}
        </span>
      </div>
      <div class="kpi" in:fly={{ y: 14, duration: 440, delay: 50, easing: cubicOut }}>
        <span class="kpi-label">{t('dash.kpi.review')}</span>
        <span class="kpi-big num">{formatSize(store.reviewBytes)}</span>
        <span class="kpi-foot" style="color: var(--color-warn)">
          {t('dash.kpi.items', [store.pendingReviewCount])}
        </span>
      </div>
      <div class="kpi" in:fly={{ y: 14, duration: 440, delay: 100, easing: cubicOut }}>
        <span class="kpi-label">{t('dash.kpi.protected')}</span>
        <span class="kpi-big num">{formatSize(store.keepBytes)}</span>
        <span class="kpi-foot">{t('dash.kpi.inUse')}</span>
      </div>
      <div class="kpi" in:fly={{ y: 14, duration: 440, delay: 150, easing: cubicOut }}>
        <span class="kpi-label">{t('dash.kpi.totalFreed')}</span>
        <span class="kpi-big num">{formatSize(store.allTimeReclaimed)}</span>
        <span class="kpi-foot">{t('dash.kpi.cleanups', [store.history.length])}</span>
      </div>
    </section>

    <div class="d-grid">
      <section class="panel" in:fly={{ y: 18, duration: 480, delay: 120, easing: cubicOut }}>
        <div class="panel-head">
          <h2 class="section-title">{t('dash.volumes')}</h2>
          <button class="btn btn-quiet btn-sm" onclick={() => store.goSub('explorer')}>
            {t('dash.browse')}
          </button>
        </div>

        <div class="vol-list">
          {#each store.volumes as vol, i}
            {@const used = vol.totalBytes - vol.availableBytes}
            {@const pct = Math.min(Math.round((used / vol.totalBytes) * 100), 100)}
            {@const tone = pct > 88 ? 'var(--color-danger)' : pct > 72 ? 'var(--color-warn)' : 'var(--color-ok)'}
            <div class="vol" style="animation-delay: {i * 70}ms">
              <div class="vol-top">
                <span class="vol-name">
                  <Icon name={vol.isRemovable ? 'externalDrive' : 'hardDrive'} size={15} style="color: {tone}" />
                  {vol.name}
                </span>
                <span class="num vol-pct" style="color: {tone}">{pct}%</span>
              </div>
              <div class="vol-track">
                <div class="vol-fill" style="width: {pct}%; background: {tone}"></div>
              </div>
              <div class="vol-meta">
                <span class="num">{formatSize(vol.availableBytes)} {t('dash.available')}</span>
                <span class="num">{formatSize(vol.totalBytes)}</span>
              </div>
            </div>
          {/each}
        </div>

        <div class="comp">
          <h3 class="comp-title">{t('dash.composition')}</h3>
          <div class="comp-bar">
            {#if store.safeBytes > 0}
              <span class="comp-seg" style="flex-grow: {store.safeBytes}; background: var(--color-ok)" title={t('dash.comp.safe')}></span>
            {/if}
            {#if store.reviewBytes > 0}
              <span class="comp-seg" style="flex-grow: {store.reviewBytes}; background: var(--color-warn)" title={t('dash.comp.review')}></span>
            {/if}
            {#if store.keepBytes > 0}
              <span class="comp-seg" style="flex-grow: {store.keepBytes}; background: var(--color-surface-3)" title={t('dash.comp.protected')}></span>
            {/if}
          </div>
          <div class="comp-legend">
            <span><i style="background: var(--color-ok)"></i>{t('dash.comp.safe')} {formatSize(store.safeBytes)}</span>
            <span><i style="background: var(--color-warn)"></i>{t('dash.comp.review')} {formatSize(store.reviewBytes)}</span>
            <span><i style="background: var(--color-surface-3)"></i>{t('dash.comp.protected')} {formatSize(store.keepBytes)}</span>
          </div>
        </div>
      </section>

      <section class="panel" in:fly={{ y: 18, duration: 480, delay: 180, easing: cubicOut }}>
        <div class="panel-head">
          <h2 class="section-title">{t('dash.routines')}</h2>
          <span class="chip">{t('dash.itemCount', [store.routines.length])}</span>
        </div>
        <div class="rt-list">
          {#each store.routines as rt}
            <div class="rt-row">
              <div class="rt-info">
                <span class="rt-name">{rt.title}</span>
                <span class="rt-cad">
                  {t(rt.cadence)} · {t('dash.average', [formatSize(rt.averageBytes)])}
                </span>
              </div>
              <span class="rt-mode" class:auto={rt.mode === 'auto'}>
                {rt.mode === 'auto' ? t('routine.mode.auto') : t('routine.mode.approve')}
              </span>
            </div>
          {:else}
            <p class="empty">{t('routines.empty2')}</p>
          {/each}
        </div>

        <div class="panel-head recent-head">
          <h2 class="section-title">{t('dash.recent')}</h2>
          <button class="btn btn-quiet btn-sm" onclick={() => store.goSub('history')}>{t('dash.all')}</button>
        </div>
        <div class="recent">
          {#each store.history.slice(0, 4) as rec}
            <div class="rec-row">
              <span class="rec-dot" class:auto={rec.automatic}></span>
              <span class="rec-titles">{rec.titles[0] ?? t('dash.defaultTitle')}</span>
              <span class="num rec-bytes">{formatSize(rec.bytes)}</span>
              <span class="rec-when">{ago(rec.atMs)}</span>
            </div>
          {:else}
            <p class="empty">{t('dash.noHistory')}</p>
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
      color-mix(in oklch, var(--color-surface) 100%, var(--color-sheen) 1.2%),
      var(--color-surface)
    );
    box-shadow: 0 18px 40px -28px var(--color-shadow);
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
      color-mix(in oklch, var(--color-surface) 100%, var(--color-sheen) 1%),
      var(--color-surface)
    );
    box-shadow: 0 18px 40px -28px var(--color-shadow);
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

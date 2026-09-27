<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { formatSize } from '../lib/format';
  import Icon from '../lib/components/Icon.svelte';
  import SettingsDialog from '../lib/components/SettingsDialog.svelte';
  import { fade, fly, scale } from 'svelte/transition';
  import { cubicOut, cubicInOut } from 'svelte/easing';
  import { tweened } from 'svelte/motion';

  let scanning = $state(false);
  const progress = tweened(0, { duration: 2200, easing: cubicInOut });
  const R = 92;
  const C = 2 * Math.PI * R;

  // measure the total label so the dashed ring always wraps it with padding
  let totalEl = $state<HTMLElement>();
  let totalW = $state(0);
  $effect(() => {
    const el = totalEl;
    if (!el) return;
    const ro = new ResizeObserver(() => (totalW = el.offsetWidth));
    ro.observe(el);
    totalW = el.offsetWidth;
    return () => ro.disconnect();
  });

  function review() {
    // select everything actionable, then enter the smart section
    store.selectedIds = new Set(
      store.insights
        .filter((i) => i.risk !== 'keep' && !store.resolvedIds.has(i.id))
        .map((i) => i.id)
    );
    store.goSub('smart');
  }

  async function startReview() {
    if (scanning) return;
    scanning = true;
    await progress.set(100);
    scanning = false;
    progress.set(0, { duration: 0 });
    review();
  }
</script>

<div class="hub" data-od-id="home-hub">
  <!-- corner meta -->
  <div class="hub-meta hub-meta-l" in:fly={{ y: -8, duration: 420, delay: 60 }}>
    <span class="hub-mark"><Icon name="spark" size={13} /></span>
    Sift AI
  </div>
  <div class="hub-meta hub-meta-r" in:fly={{ y: -8, duration: 420, delay: 60 }}>
    <span class="engine-dot"></span>
    ENGINE ACTIVE
    <button
      type="button"
      class="hub-settings-btn"
      aria-label="系统设置"
      title="系统设置"
      onclick={() => (store.settingsOpen = true)}
      data-od-id="home-settings"
    >
      <Icon name="settings" size={14} />
    </button>
  </div>

  <!-- center stage -->
  <div class="hub-center">
    <div
      class="ring-stage"
      class:scanning
      in:fade={{ duration: 640, delay: 120 }}
      style:width={`${Math.min(560, Math.max(320, (totalW + 56) / 0.8))}px`}
      aria-hidden="true"
    >
      {#if scanning}
        <svg class="prog-ring" viewBox="0 0 200 200">
          <circle class="prog-track" cx="100" cy="100" r={R} />
          <circle
            class="prog-arc"
            cx="100"
            cy="100"
            r={R}
            style:stroke-dashoffset={C * (1 - $progress / 100)}
            transform="rotate(-90 100 100)"
          />
        </svg>
      {:else}
        <svg class="ring ring-track" viewBox="0 0 200 200">
          <circle cx="100" cy="100" r="92" />
        </svg>
        <svg class="ring ring-arc" viewBox="0 0 200 200">
          <defs>
            <linearGradient id="ringGrad" x1="0" y1="0" x2="1" y2="1">
              <stop offset="0%" stop-color="var(--color-violet)" stop-opacity="0.15" />
              <stop offset="45%" stop-color="var(--color-violet)" stop-opacity="0.9" />
              <stop offset="100%" stop-color="var(--color-accent-hi)" stop-opacity="0.55" />
            </linearGradient>
          </defs>
          <circle
            cx="100"
            cy="100"
            r="92"
            fill="none"
            stroke="url(#ringGrad)"
            stroke-width="2.4"
            stroke-linecap="round"
            stroke-dasharray="432 146"
            transform="rotate(-90 100 100)"
          />
        </svg>
        <svg class="ring ring-dash" viewBox="0 0 200 200">
          <circle
            cx="100"
            cy="100"
            r="80"
            fill="none"
            stroke="var(--color-faint)"
            stroke-width="1"
            stroke-dasharray="2 7"
          />
        </svg>
      {/if}
      <span class="ring-glow"></span>
    </div>

    <p class="hub-eyebrow" in:fade={{ duration: 480, delay: 120 }}>{scanning ? 'SCANNING' : 'TOTAL'}</p>
    {#if scanning}
      <h1 class="hub-total num">
        {Math.round($progress)}<span class="hub-pct">%</span>
      </h1>
    {:else}
      <h1 class="hub-total num" bind:this={totalEl} in:fly={{ y: 16, duration: 620, delay: 180, easing: cubicOut }}>
        {formatSize(store.totalReclaimable)}
      </h1>
    {/if}
    <p class="hub-state" class:state-scanning={scanning}>
      <span class="state-dot"></span>
      {scanning ? '正在检查磁盘…' : '已达最佳整理状态'}
    </p>

    <button
      class="hub-cta"
      onclick={startReview}
      disabled={scanning}
      in:scale={{ duration: 480, delay: 380, easing: cubicOut }}
      data-od-id="home-review-cta"
    >
      {scanning ? '正在检查' : '检查并整理'}
      {#if !scanning}<Icon name="arrowRight" size={15} />{/if}
    </button>
  </div>

  <!-- entry cards -->
  <div class="hub-cards">
    <button
      class="hub-card"
      onclick={() => store.go('dashboard')}
      in:fly={{ y: 26, duration: 520, delay: 420, easing: cubicOut }}
      data-od-id="home-card-dashboard"
    >
      <span class="card-glyph glyph-blue"><Icon name="gauge" size={19} /></span>
      <span class="card-title">仪表盘</span>
      <span class="card-big num">{formatSize(store.totalReclaimable)}</span>
      <span class="card-sub">磁盘健康与总览</span>
    </button>

    <button
      class="hub-card"
      onclick={() => store.goSub('smart')}
      in:fly={{ y: 26, duration: 520, delay: 480, easing: cubicOut }}
      data-od-id="home-card-suggestions"
    >
      <span class="card-glyph glyph-violet"><Icon name="brain" size={19} /></span>
      <span class="card-title">智能建议</span>
      <span class="card-big num">{formatSize(store.safeReclaimable)}</span>
      <span class="card-sub">AI 推荐删除 · {store.pendingReviewCount} 项待确认</span>
    </button>

    <button
      class="hub-card"
      onclick={() => store.goSub('explorer')}
      in:fly={{ y: 26, duration: 520, delay: 540, easing: cubicOut }}
      data-od-id="home-card-explorer"
    >
      <span class="card-glyph glyph-blue"><Icon name="layers" size={19} /></span>
      <span class="card-title">磁盘浏览</span>
      <span class="card-big">浏览</span>
      <span class="card-sub">手动目录清理与下钻</span>
    </button>

    <button
      class="hub-card"
      onclick={() => store.goSub('history')}
      in:fly={{ y: 26, duration: 520, delay: 620, easing: cubicOut }}
      data-od-id="home-card-history"
    >
      <span class="card-glyph glyph-emerald"><Icon name="clock" size={19} /></span>
      <span class="card-title">清理历史</span>
      <span class="card-big num">{formatSize(store.allTimeReclaimed)}</span>
      <span class="card-sub">回顾已完成的优化</span>
    </button>
  </div>

  <SettingsDialog />
</div>

<style>
  .hub {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 30px 48px 40px;
    overflow: hidden;
  }

  .hub-meta {
    position: absolute;
    top: 22px;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.14em;
    color: var(--color-faint);
    font-family: var(--font-mono);
  }
  .hub-meta-l {
    left: 44px;
    letter-spacing: 0.06em;
  }
  .hub-meta-r {
    right: 44px;
  }
  .hub-mark {
    display: inline-flex;
    color: var(--color-violet);
  }
  .engine-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--color-ok);
    box-shadow: 0 0 8px var(--color-ok);
  }
  .hub-settings-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    margin-left: 2px;
    padding: 0;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--color-faint);
    cursor: default;
    transition: background 0.15s ease, color 0.15s ease;
  }
  .hub-settings-btn:hover {
    background: color-mix(in oklch, var(--color-surface-3, var(--color-surface-2)) 70%, transparent);
    color: var(--color-fg);
  }
  .hub-settings-btn:focus-visible {
    outline: 2px solid color-mix(in oklch, var(--color-accent) 70%, transparent);
    outline-offset: 2px;
  }

  .hub-center {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    margin-top: -26px;
  }

  .ring-stage {
    position: absolute;
    top: 50%;
    left: 50%;
    width: clamp(320px, 32vw, 440px);
    aspect-ratio: 1;
    transform: translate(-50%, -54%);
    pointer-events: none;
  }
  .ring {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    overflow: visible;
  }
  .ring-track circle {
    fill: none;
    stroke: color-mix(in oklch, var(--color-border-strong) 40%, transparent);
    stroke-width: 1;
  }
  .ring-arc {
    transform-origin: 50% 50%;
    animation: ring-spin 72s linear infinite;
  }
  .ring-dash {
    transform-origin: 50% 50%;
    animation: ring-spin 38s linear infinite reverse;
    opacity: 0.7;
  }
  .ring-glow {
    position: absolute;
    inset: 16%;
    border-radius: 50%;
    background: radial-gradient(
      circle,
      color-mix(in oklch, var(--color-violet) 13%, transparent),
      transparent 68%
    );
    filter: blur(6px);
  }

  .prog-ring {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    overflow: visible;
  }
  .prog-track {
    fill: none;
    stroke: color-mix(in oklch, var(--color-border-strong) 34%, transparent);
    stroke-width: 3;
  }
  .prog-arc {
    fill: none;
    stroke: var(--color-violet);
    stroke-width: 3;
    stroke-linecap: round;
    transition: stroke-dashoffset linear;
    filter: drop-shadow(0 0 7px color-mix(in oklch, var(--color-violet) 70%, transparent));
  }
  .ring-stage.scanning .ring-glow {
    inset: 8%;
    background: radial-gradient(
      circle,
      color-mix(in oklch, var(--color-violet) 18%, transparent),
      transparent 70%
    );
  }
  .hub-pct {
    font-size: 0.42em;
    font-weight: 600;
    margin-left: 4px;
    color: var(--color-faint);
  }
  .state-scanning {
    color: var(--color-violet);
  }
  .state-scanning .state-dot {
    background: var(--color-violet);
    box-shadow: 0 0 8px var(--color-violet);
    animation: state-pulse 1.1s ease-in-out infinite;
  }
  @keyframes state-pulse {
    50% {
      opacity: 0.45;
    }
  }
  @keyframes ring-spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .ring-arc,
    .ring-dash {
      animation: none;
    }
  }
  .hub-center > :not(.ring-stage) {
    position: relative;
    z-index: 1;
  }
  .hub-eyebrow {
    margin: 0 0 10px;
    font-family: var(--font-mono);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.32em;
    color: var(--color-faint);
  }
  .hub-total {
    margin: 0;
    font-size: clamp(76px, 9vw, 128px);
    font-weight: 650;
    line-height: 1;
    letter-spacing: -0.04em;
    background: linear-gradient(180deg, #fff 20%, color-mix(in oklch, #fff 62%, var(--color-bg)));
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
  }
  .hub-state {
    margin: 16px 0 30px;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-size: 12.5px;
    font-weight: 550;
    color: var(--color-ok);
    font-family: var(--font-mono);
    letter-spacing: 0.04em;
  }
  .state-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--color-ok);
    box-shadow: 0 0 8px var(--color-ok);
  }

  .hub-cta {
    display: inline-flex;
    align-items: center;
    gap: 9px;
    height: 46px;
    padding: 0 30px;
    border: none;
    border-radius: 14px;
    background: #f4f4f6;
    color: #141418;
    font-size: 14px;
    font-weight: 600;
    cursor: default;
    box-shadow:
      0 1px 0 rgba(255, 255, 255, 0.6) inset,
      0 18px 40px -16px rgba(0, 0, 0, 0.8);
    transition: transform 0.16s cubic-bezier(0.22, 1, 0.36, 1), box-shadow 0.16s ease;
  }
  .hub-cta:hover {
    transform: translateY(-2px);
    box-shadow:
      0 1px 0 rgba(255, 255, 255, 0.6) inset,
      0 26px 52px -18px rgba(0, 0, 0, 0.85);
  }
  .hub-cta:active {
    transform: translateY(0);
  }
  .hub-cta:disabled {
    opacity: 0.72;
    cursor: default;
  }
  .hub-cta:focus-visible {
    outline: 2px solid color-mix(in oklch, var(--color-accent) 70%, transparent);
    outline-offset: 3px;
  }

  .hub-cards {
    position: absolute;
    bottom: 40px;
    left: 48px;
    right: 48px;
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 14px;
  }
  .hub-card {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 5px;
    padding: 20px 22px 21px;
    border-radius: 18px;
    border: 1px solid color-mix(in oklch, var(--color-border-strong) 45%, transparent);
    background: linear-gradient(
      180deg,
      color-mix(in oklch, var(--color-surface) 88%, white 1.2%),
      color-mix(in oklch, var(--color-surface) 94%, black 8%)
    );
    box-shadow:
      0 1px 0 color-mix(in oklch, white 4%, transparent) inset,
      0 22px 46px -26px black;
    cursor: default;
    text-align: left;
    transition: transform 0.2s cubic-bezier(0.22, 1, 0.36, 1), border-color 0.2s ease,
      box-shadow 0.2s ease;
  }
  .hub-card:hover {
    transform: translateY(-3px);
    border-color: color-mix(in oklch, var(--color-border-strong) 75%, transparent);
    box-shadow:
      0 1px 0 color-mix(in oklch, white 6%, transparent) inset,
      0 32px 60px -26px black;
  }
  .hub-card:active {
    transform: translateY(-1px);
  }
  .hub-card:focus-visible {
    outline: 2px solid color-mix(in oklch, var(--color-accent) 70%, transparent);
    outline-offset: 3px;
  }

  .card-glyph {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 38px;
    height: 38px;
    border-radius: 11px;
    margin-bottom: 9px;
  }
  .glyph-violet {
    color: var(--color-violet);
    background: color-mix(in oklch, var(--color-violet) 15%, transparent);
  }
  .glyph-blue {
    color: var(--color-accent-hi);
    background: color-mix(in oklch, var(--color-accent) 15%, transparent);
  }
  .glyph-emerald {
    color: var(--color-ok);
    background: color-mix(in oklch, var(--color-ok) 14%, transparent);
  }

  .card-title {
    font-family: var(--font-mono);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.18em;
    color: var(--color-faint);
  }
  .card-big {
    font-size: 24px;
    font-weight: 620;
    letter-spacing: -0.02em;
    color: var(--color-fg);
    line-height: 1.15;
  }
  .card-sub {
    font-size: 12px;
    color: var(--color-muted);
  }
</style>

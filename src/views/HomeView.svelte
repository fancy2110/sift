<script lang="ts">
  import { store } from '../lib/store.svelte';
  import { formatSize } from '../lib/format';
  import Icon from '../lib/components/Icon.svelte';
  import SettingsDialog from '../lib/components/SettingsDialog.svelte';
  import { fade, fly, scale } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { t } from '../lib/i18n.svelte';

  let reviewRequested = $state(false);

  // Measure the whole text stack so the dashed ring wraps everything (eyebrow,
  // number, state, button) with generous padding — adapting to the real content.
  let stackEl = $state<HTMLElement>();
  let centerEl = $state<HTMLElement>();
  let stackW = $state(280);
  let stackH = $state(260);
  let centerW = $state(900);
  let centerH = $state(560);
  $effect(() => {
    const stack = stackEl;
    const center = centerEl;
    if (!stack || !center) return;
    const measure = () => {
      stackW = stack.offsetWidth;
      stackH = stack.offsetHeight;
      centerW = center.clientWidth;
      centerH = center.clientHeight;
    };
    const ro = new ResizeObserver(measure);
    ro.observe(stack);
    ro.observe(center);
    measure();
    return () => ro.disconnect();
  });

  // Ring fills the largest square the stage allows (width AND height bounded),
  // so it is never clipped. The text stack is then scaled to fit inside the
  // dashed ring's inner diameter (r=80 → 80% of stage) with breathing padding,
  // which structurally guarantees the label never covers the ring.
  // The ring stage is built at the "natural" size: stack's largest dimension
  // plus equal padding on every side. Rings and stack live in the SAME box and
  // scale together as one composition, so the ring always wraps the text.
  // Only when the natural box exceeds the available stage do we shrink the
  // whole thing (no floor) — the wrap relationship is preserved exactly.
  // The ring stage is sized so the stack's largest dimension fills at most 74%
  // of the box — the dashed ring sits at 80%, so the text always lands inside
  // it with a proportional margin at any content size. Rings and stack live in
  // the SAME box and scale together as one composition; when the natural box
  // exceeds the available stage, the whole thing shrinks (no floor), preserving
  // the wrap relationship exactly.
  let desiredW = $derived(Math.max(stackW, stackH) / 0.74);
  let fitScale = $derived(
    Math.min(1, (Math.min(centerW, centerH) - 8) / desiredW)
  );

  // Once a review-triggered scan finishes and produces findings, enter smart.
  $effect(() => {
    if (reviewRequested && !store.scanning && store.hasFindings) {
      reviewRequested = false;
      store.goSub('smart');
    }
  });

  async function handleCta() {
    if (store.scanning) return;
    if (store.hasFindings) {
      store.goSub('smart');
      return;
    }
    reviewRequested = true;
    const preferred = store.volumes.find((v) => !v.isRemovable) ?? store.volumes[0];
    if (preferred) await store.selectDisk(preferred.id);
  }
</script>

<div class="hub">
  <!-- corner meta -->
  <div class="hub-meta hub-meta-l" in:fly={{ y: -8, duration: 420, delay: 60 }}>
    <span class="hub-mark"><Icon name="spark" size={13} /></span>
    {t('home.siftAi')}
  </div>
  <div class="hub-meta hub-meta-r" in:fly={{ y: -8, duration: 420, delay: 60 }}>
    <span class="engine-dot"></span>
    {t('home.engineActive')}
    <button
      type="button"
      class="hub-settings-btn"
      aria-label={t('home.settings')}
      title={t('home.settings')}
      onclick={() => (store.settingsOpen = true)}
    >
      <Icon name="settings" size={14} />
    </button>
  </div>

  <!-- center stage -->
  <div class="hub-center" bind:this={centerEl}>
   <div
     class="hub-comp"
     style:width={`${desiredW}px`}
     style:height={`${desiredW}px`}
     style:transform={`scale(${fitScale})`}
   >
    <div
      class="ring-stage"
      class:scanning={store.scanning}
      in:fade={{ duration: 640, delay: 120 }}
      aria-hidden="true"
    >
      {#if store.scanning}
        <svg class="prog-ring spin-slow" viewBox="0 0 200 200">
          <circle class="prog-track" cx="100" cy="100" r="92" />
          <circle
            class="prog-arc"
            cx="100"
            cy="100"
            r="92"
            stroke-dasharray="260 318"
            transform="rotate(-90 100 100)"
          />
        </svg>
      {:else}
        <svg class="ring-svg ring-track" viewBox="0 0 200 200">
          <circle cx="100" cy="100" r="92" />
        </svg>
        <svg class="ring-svg ring-arc" viewBox="0 0 200 200">
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
        <svg class="ring-svg ring-dash" viewBox="0 0 200 200">
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

    <div
      class="hub-stack"
      bind:this={stackEl}
    >
    <p class="hub-eyebrow" in:fade={{ duration: 480, delay: 120 }}>
      {store.scanning ? t('home.scanning') : t('home.total')}
    </p>
    {#if store.scanning}
      <h1 class="hub-total num">
        {store.scannedFiles.toLocaleString()}<span class="hub-pct">{t('home.itemsUnit')}</span>
      </h1>
    {:else}
      <h1 class="hub-total num" in:fly={{ y: 16, duration: 620, delay: 180, easing: cubicOut }}>
        {formatSize(store.totalReclaimable)}
      </h1>
    {/if}
    <p
      class="hub-state"
      class:state-scanning={store.scanning}
    >
      <span class="state-dot"></span>
      {store.scanning ? t('home.stateScanning') : t('home.stateReady')}
    </p>

    <button
      class="hub-cta"
      onclick={handleCta}
      disabled={store.scanning}
      in:scale={{ duration: 480, delay: 380, easing: cubicOut }}
    >
      {store.scanning ? t('home.ctaScanning') : t('home.ctaScan')}
    </button>
    </div>
   </div>
  </div>

  <!-- entry cards -->
  <div class="hub-cards">
    <button class="hub-card" onclick={() => store.go('dashboard')} in:fly={{ y: 26, duration: 520, delay: 420, easing: cubicOut }}>
      <span class="card-glyph glyph-blue"><Icon name="gauge" size={19} /></span>
      <span class="card-title">{t('home.card.dashboard')}</span>
      <span class="card-big num">{formatSize(store.totalReclaimable)}</span>
      <span class="card-sub">{t('home.card.dashboardSub')}</span>
    </button>

    <button class="hub-card" onclick={() => store.goSub('smart')} in:fly={{ y: 26, duration: 520, delay: 480, easing: cubicOut }}>
      <span class="card-glyph glyph-violet"><Icon name="brain" size={19} /></span>
      <span class="card-title">{t('home.card.smart')}</span>
      <span class="card-big num">{formatSize(store.safeReclaimable)}</span>
      <span class="card-sub">{t('home.card.smartSub', [store.pendingReviewCount])}</span>
    </button>

    <button class="hub-card" onclick={() => store.goSub('explorer')} in:fly={{ y: 26, duration: 520, delay: 540, easing: cubicOut }}>
      <span class="card-glyph glyph-blue"><Icon name="layers" size={19} /></span>
      <span class="card-title">{t('home.card.explorer')}</span>
      <span class="card-big">{t('home.browse')}</span>
      <span class="card-sub">{t('home.card.explorerSub')}</span>
    </button>

    <button class="hub-card" onclick={() => store.goSub('history')} in:fly={{ y: 26, duration: 520, delay: 620, easing: cubicOut }}>
      <span class="card-glyph glyph-emerald"><Icon name="clock" size={19} /></span>
      <span class="card-title">{t('home.card.history')}</span>
      <span class="card-big num">{formatSize(store.allTimeReclaimed)}</span>
      <span class="card-sub">{t('home.card.historySub')}</span>
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
    padding: 74px 56px 34px;
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
    flex: 1;
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
  }

  .hub-comp {
    position: relative;
    flex-shrink: 0;
    transform-origin: 50% 50%;
    transition: transform 0.24s cubic-bezier(0.22, 1, 0.36, 1);
  }
  .ring-stage {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }
  .ring-svg {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    overflow: visible;
  }
  .ring-track circle {
    fill: none;
    stroke: none;
    stroke-width: 0;
  }
  .ring-arc {
    transform-origin: 50% 50%;
    animation: ring-spin 120s linear infinite;
  }
  .ring-dash {
    transform-origin: 50% 50%;
    animation: ring-spin-rev 64s linear infinite;
    opacity: 0.55;
  }
  .ring-glow {
    position: absolute;
    inset: 16%;
    border-radius: 50%;
    background: radial-gradient(
      circle,
      color-mix(in oklch, var(--color-violet) 9%, transparent),
      transparent 70%
    );
    filter: blur(10px);
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
    filter: drop-shadow(0 0 7px color-mix(in oklch, var(--color-violet) 70%, transparent));
  }
  .spin-slow {
    animation: ring-spin 1.3s linear infinite;
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
  @keyframes ring-spin-rev {
    to {
      transform: rotate(-360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .ring-arc,
    .ring-dash,
    .spin-slow {
      animation: none;
    }
  }
  .hub-center > :not(.ring-stage) {
    position: relative;
    z-index: 1;
  }
  .hub-stack {
    position: absolute;
    top: 50%;
    left: 50%;
    display: flex;
    flex-direction: column;
    align-items: center;
    transform: translate(-50%, -50%);
    transform-origin: 50% 50%;
  }
  .hub-eyebrow {
    margin: 0 0 14px;
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
    background: linear-gradient(
      180deg,
      var(--color-fg) 20%,
      color-mix(in oklch, var(--color-fg) 62%, var(--color-bg))
    );
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
  }
  .hub-state {
    margin: 20px 0 36px;
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
    background: var(--color-cta-bg);
    color: var(--color-cta-fg);
    font-size: 14px;
    font-weight: 600;
    cursor: default;
    box-shadow:
      0 1px 0 color-mix(in oklch, var(--color-sheen) 60%, transparent) inset,
      0 18px 40px -16px var(--color-shadow);
    transition: transform 0.16s cubic-bezier(0.22, 1, 0.36, 1), box-shadow 0.16s ease;
  }
  .hub-cta:hover {
    transform: translateY(-2px);
    box-shadow:
      0 1px 0 color-mix(in oklch, var(--color-sheen) 60%, transparent) inset,
      0 26px 52px -18px var(--color-shadow);
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
    flex-shrink: 0;
    width: 100%;
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 16px;
  }
  .hub-card {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    padding: 22px 24px 23px;
    border-radius: 18px;
    border: 1px solid color-mix(in oklch, var(--color-border-strong) 45%, transparent);
    background: linear-gradient(
      180deg,
      color-mix(in oklch, var(--color-surface) 88%, var(--color-sheen) 1.2%),
      color-mix(in oklch, var(--color-surface) 94%, var(--color-shadow) 8%)
    );
    box-shadow:
      0 1px 0 color-mix(in oklch, var(--color-sheen) 4%, transparent) inset,
      0 22px 46px -26px var(--color-shadow);
    cursor: default;
    text-align: left;
    animation: card-glow 11s ease-in-out infinite;
    transition: transform 0.28s cubic-bezier(0.22, 1, 0.36, 1),
      border-color 0.2s ease, box-shadow 0.28s ease;
  }
  .hub-card:nth-child(2) {
    animation-duration: 13s;
    animation-delay: -3s;
  }
  .hub-card:nth-child(3) {
    animation-duration: 9.5s;
    animation-delay: -6s;
  }
  .hub-card:nth-child(4) {
    animation-duration: 12s;
    animation-delay: -1.5;
  }
  /* static cards with a very subtle brightness breathing */
  @keyframes card-glow {
    0%,
    100% {
      filter: brightness(1);
    }
    50% {
      filter: brightness(1.022);
    }
  }
  .hub-card:hover {
    border-color: color-mix(in oklch, var(--color-border-strong) 75%, transparent);
    box-shadow:
      0 1px 0 color-mix(in oklch, var(--color-sheen) 6%, transparent) inset,
      0 30px 58px -26px var(--color-shadow);
    transform: translateY(-2px);
  }
  .hub-card:active {
    transform: translateY(0);
  }
  @media (prefers-reduced-motion: reduce) {
    .hub-card {
      animation: none;
    }
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
    transition: background 0.28s ease;
  }
  @media (prefers-reduced-motion: reduce) {
    .card-glyph {
      transition: none;
    }
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

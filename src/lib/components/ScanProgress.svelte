<script lang="ts">
  import { store } from '../store.svelte';
  import { formatSize } from '../format';

  let { onCancel }: { onCancel: () => void } = $props();

  const calibrating = $derived(store.calibrating && !store.scanning);
  const title = $derived(calibrating ? '正在后台校准大型目录' : '正在扫描');
  const progressPct = $derived(Math.round(store.scanCoverage * 100));

  const stats = $derived([
    { label: '文件', value: store.scannedFiles.toLocaleString() },
    { label: '目录', value: store.scannedDirs.toLocaleString() },
    { label: '已扫描', value: formatSize(store.scannedBytes, 1) },
    { label: '速率', value: `${formatSize(store.scanRateBytes, 1)}/s` },
    { label: '耗时', value: `${store.scanElapsed.toFixed(1)}s` }
  ]);
</script>

<div class="progress-card" role="status" aria-live="polite">
  <div class="progress-head">
    <span class="dot {calibrating ? 'amber' : ''}" aria-hidden="true"></span>
    <span class="title">{title}</span>
    {#if calibrating}
      <span class="cal-count">已校准 {store.calibratedCount}</span>
    {/if}
    <button class="cancel" onclick={onCancel} type="button">取消</button>
  </div>

  {#if !calibrating}
    <div class="bar-track" aria-label={`扫描进度 ${progressPct}%`}>
      <div class="bar-fill" style={`width: ${progressPct}%`}></div>
    </div>
    <span class="pct">{progressPct}%</span>
  {/if}

  <div class="progress-grid">
    {#each stats as s}
      <div class="stat">
        <span class="value">{s.value}</span>
        <span class="label">{s.label}</span>
      </div>
    {/each}
  </div>
</div>

<style>
  .progress-card {
    position: absolute;
    top: 12px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 20;
    min-width: 440px;
    padding: 12px 16px;
    border-radius: 14px;
    background: rgba(22, 22, 28, 0.82);
    backdrop-filter: blur(18px);
    border: 1px solid rgba(255, 255, 255, 0.1);
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.45);
    color: #f2f2f5;
    animation: drop 0.22s ease-out;
  }
  @keyframes drop {
    from { opacity: 0; transform: translateX(-50%) translateY(-8px); }
    to { opacity: 1; transform: translateX(-50%) translateY(0); }
  }
  .progress-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 10px;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #4da3ff;
    box-shadow: 0 0 0 0 rgba(77, 163, 255, 0.6);
    animation: pulse 1.4s infinite;
  }
  .dot.amber {
    background: #f5a524;
    box-shadow: 0 0 0 0 rgba(245, 165, 36, 0.6);
    animation: pulse-amber 1.4s infinite;
  }
  @keyframes pulse {
    0% { box-shadow: 0 0 0 0 rgba(77, 163, 255, 0.55); }
    70% { box-shadow: 0 0 0 8px rgba(77, 163, 255, 0); }
    100% { box-shadow: 0 0 0 0 rgba(77, 163, 255, 0); }
  }
  @keyframes pulse-amber {
    0% { box-shadow: 0 0 0 0 rgba(245, 165, 36, 0.55); }
    70% { box-shadow: 0 0 0 8px rgba(245, 165, 36, 0); }
    100% { box-shadow: 0 0 0 0 rgba(245, 165, 36, 0); }
  }
  .title {
    font-size: 13px;
    font-weight: 600;
    flex: 1;
  }
  .cal-count {
    font-size: 11px;
    color: rgba(242, 242, 245, 0.65);
    font-variant-numeric: tabular-nums;
  }
  .cancel {
    border: none;
    background: rgba(255, 255, 255, 0.12);
    color: #f2f2f5;
    font-size: 12px;
    padding: 4px 12px;
    border-radius: 8px;
    cursor: pointer;
    transition: background 0.15s;
  }
  .cancel:hover {
    background: rgba(255, 90, 90, 0.5);
  }
  .bar-track {
    height: 6px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.12);
    overflow: hidden;
  }
  .bar-fill {
    height: 100%;
    border-radius: 999px;
    background: linear-gradient(90deg, #4da3ff, #7cc4ff);
    transition: width 0.25s ease-out;
  }
  .pct {
    display: block;
    margin-top: 6px;
    font-size: 11px;
    color: rgba(242, 242, 245, 0.6);
    font-variant-numeric: tabular-nums;
  }
  .progress-grid {
    display: grid;
    grid-template-columns: repeat(5, 1fr);
    gap: 12px;
    margin-top: 10px;
  }
  .stat {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .value {
    font-size: 15px;
    font-weight: 650;
    font-variant-numeric: tabular-nums;
  }
  .label {
    font-size: 11px;
    color: rgba(242, 242, 245, 0.55);
  }
</style>

<script lang="ts">
  import TitleBar from './lib/components/TitleBar.svelte';
  import Toasts from './lib/components/Toasts.svelte';
  import ScanOverlay from './lib/components/ScanOverlay.svelte';
  import CleanerView from './views/CleanerView.svelte';
  import { store } from './lib/store.svelte';

  let scanning = $state(false);
  let scanProgress = $state(0);
  let scanStage = $state(0);

  async function runScan() {
    if (scanning) return;
    scanning = true;
    scanProgress = 0;
    scanStage = 0;
    const started = Date.now();
    const duration = 2200;
    while (Date.now() - started < duration) {
      await new Promise((r) => setTimeout(r, 80));
      const t = Math.min((Date.now() - started) / duration, 1);
      scanProgress = Math.round(t * 100);
      scanStage = Math.min(Math.floor(t * 5), 4);
    }
    scanning = false;
    if (store.autoOn) {
      await store.runAuto();
    } else {
      store.toast('分析完成，点击右侧标签查看清理候选');
    }
  }
</script>

<div class="flex h-full flex-col">
  <TitleBar onScan={runScan} {scanning} />
  <main class="relative min-h-0 flex-1">
    <CleanerView />
  </main>
</div>

<Toasts />
{#if scanning}
  <ScanOverlay progress={scanProgress} stage={scanStage} />
{/if}

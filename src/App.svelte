<script lang="ts">
  import TitleBar from './lib/components/TitleBar.svelte';
  import Toasts from './lib/components/Toasts.svelte';
  import CleanerView from './views/CleanerView.svelte';
  import { store } from './lib/store.svelte';

  store.init();

  function runScan() {
    if (store.currentVolumeId) store.selectVolume(store.currentVolumeId);
  }
</script>

<div class="flex h-full flex-col">
  <TitleBar onScan={runScan} scanning={store.scanning} />
  <main class="relative min-h-0 flex-1">
    <CleanerView />
  </main>
</div>

<Toasts />

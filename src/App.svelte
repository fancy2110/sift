<script lang="ts">
  import Toasts from './lib/components/Toasts.svelte';
  import PermissionDrawer from './lib/components/PermissionDrawer.svelte';
  import HomeView from './views/HomeView.svelte';
  import DashboardView from './views/DashboardView.svelte';
  import SubPageView from './views/SubPageView.svelte';
  import { store } from './lib/store.svelte';
  import { initI18n } from './lib/i18n.svelte';
  import { fade } from 'svelte/transition';

  initI18n().then(() => store.init());
</script>

<div class="flex h-full flex-col">
  <main class="relative min-h-0 flex-1">
    {#key store.view}
      {#if store.view === 'home'}
        <div class="absolute inset-0" in:fade={{ duration: 280 }}>
          <HomeView />
        </div>
      {:else if store.view === 'dashboard'}
        <div class="absolute inset-0" in:fade={{ duration: 280 }}>
          <DashboardView />
        </div>
      {:else}
        <div class="absolute inset-0" in:fade={{ duration: 280 }}>
          <SubPageView />
        </div>
      {/if}
    {/key}
  </main>
</div>

<Toasts />
{#if store.view === 'home'}
  <PermissionDrawer />
{/if}

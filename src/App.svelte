<script lang="ts">
  import Toasts from './lib/components/Toasts.svelte';
  import PermissionDrawer from './lib/components/PermissionDrawer.svelte';
  import HomeView from './views/HomeView.svelte';
  import DashboardView from './views/DashboardView.svelte';
  import SubPageView from './views/SubPageView.svelte';
  import { store } from './lib/store.svelte';
  import { initI18n, t } from './lib/i18n.svelte';
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
{#if store.browserMode}
  <div
    class="pointer-events-none fixed bottom-6 left-6 z-50 flex items-center gap-2 rounded-full px-3 py-1.5 text-xs font-medium"
    role="status"
    style="border: 1px solid color-mix(in oklch, var(--color-amber, #c8852c) 26%, transparent); background: var(--color-surface); color: var(--color-amber, #c8852c);"
  >
    <span class="h-1.5 w-1.5 rounded-full" style="background: var(--color-amber, #c8852c);"></span>
    {t('banner.browserMode')}
  </div>
{/if}

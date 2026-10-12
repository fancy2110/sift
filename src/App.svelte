<script lang="ts">
  import Toasts from './lib/components/Toasts.svelte';
  import HomeView from './views/HomeView.svelte';
  import { store } from './lib/store.svelte';
  import { initI18n, t } from './lib/i18n.svelte';
  import { fade } from 'svelte/transition';
  import type { Component } from 'svelte';
  import { startupMark } from './lib/startup';

  // Startup path: the home screen is the only view in the initial chunk. The
  // dashboard and the heavy sub page (smart/explorer/history/routines) load on
  // demand, so first paint never pays for their parse/compile cost.
  const loaders = {
    home: () => import('./views/HomeView.svelte'),
    dashboard: () => import('./views/DashboardView.svelte'),
    sub: () => import('./views/SubPageView.svelte')
  } as const;

  let viewComponents = $state<Record<string, Component>>({ home: HomeView });
  $effect(() => {
    const name = store.view;
    if (viewComponents[name]) return;
    loaders[name]().then((module) => {
      viewComponents = { ...viewComponents, [name]: module.default };
      startupMark(`view:${name}-loaded`);
    });
  });

  // The permission drawer is only relevant during an active scan; lazy load it
  // together with scan state instead of bundling it into the first paint.
  let PermissionDrawer = $state<Component | null>(null);
  $effect(() => {
    if (store.view === 'home' && store.scanning && !PermissionDrawer) {
      import('./lib/components/PermissionDrawer.svelte').then((module) => {
        PermissionDrawer = module.default;
      });
    }
  });

  startupMark('webview:app-script');
  // i18n and store bootstrap are independent: run them concurrently instead
  // of serializing two IPC round trips. The default dictionary is already zh,
  // so the first frame is correct without waiting for `get_language`.
  initI18n().then(() => startupMark('webview:i18n-ready'));
  store.init().then(() => startupMark('webview:init-done'));
</script>

<div class="flex h-full flex-col">
  <main class="relative min-h-0 flex-1">
    {#if store.view === 'home'}
      <!-- No in:fade on the home screen: its own intro transition (skipped on
           cold boot via firstBoot) decides motion; a wrapper fade would keep
           the whole first frame at opacity 0. -->
      <div class="absolute inset-0">
        <HomeView />
      </div>
    {:else if store.view === 'dashboard'}
      <div class="absolute inset-0" in:fade={{ duration: 280 }}>
        {#if viewComponents.dashboard}
          {@const DashboardView = viewComponents.dashboard}
          <DashboardView />
        {/if}
      </div>
    {:else}
      <div class="absolute inset-0" in:fade={{ duration: 280 }}>
        {#if viewComponents.sub}
          {@const SubPageView = viewComponents.sub}
          <SubPageView />
        {/if}
      </div>
    {/if}
  </main>
</div>

<Toasts />
{#if store.view === 'home' && PermissionDrawer}
  {@const Drawer = PermissionDrawer}
  <Drawer />
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

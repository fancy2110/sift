<script lang="ts">
  /**
   * Single-list browser: always shows the current directory. The top bar
   * carries the volume picker, an up button and a breadcrumb path; the header
   * above the list explains the directory with the model. Works without a scan.
   */
  import Icon from './Icon.svelte';
  import LocationPicker from './LocationPicker.svelte';
  import { store } from '../store.svelte';
  import { formatSize } from '../format';
  import { t } from '../i18n.svelte';
  import { onMount } from 'svelte';

  let pathBar = $state<HTMLElement>();

  onMount(() => {
    if (!store.explorerPath) void store.openExplorer();
  });

  // Re-read the directory whenever the tab becomes active, so sizes and the
  // largest-first order are fresh each time.
  let wasActive = false;
  $effect(() => {
    const active = store.subTab === 'explorer';
    if (active && !wasActive && store.explorerPath) {
      void store.refreshExplorer();
    }
    wasActive = active;
  });

  // Keep the current (last) segment visible when the path is long.
  $effect(() => {
    const path = store.explorerPath;
    if (path && pathBar) pathBar.scrollLeft = pathBar.scrollWidth;
  });

  const currentName = $derived(
    store.explorerPath.split('/').filter(Boolean).pop() ?? '/'
  );
  const volume = $derived(
    [...store.volumes]
      .filter((item) => store.explorerPath.startsWith(item.mountPoint))
      .sort((a, b) => b.mountPoint.length - a.mountPoint.length)[0] ?? null
  );
  const crumbs = $derived.by(() => {
    const mount = volume ? volume.mountPoint.replace(/\/$/, '') : '';
    const parts = (mount ? store.explorerPath.slice(mount.length) : store.explorerPath)
      .split('/')
      .filter(Boolean);
    // The root is always the first item, so the trail can walk back up.
    const out: { label: string; path: string }[] = [
      { label: '/', path: volume?.mountPoint ?? '/' }
    ];
    let acc = mount;
    for (const part of parts) {
      acc += `/${part}`;
      out.push({ label: part, path: acc });
    }
    return out;
  });
  const sortedEntries = $derived(
    [...store.explorerRight].sort(
      (a, b) => b.size - a.size || Number(b.isDir) - Number(a.isDir)
    )
  );
  const totalBytes = $derived(
    store.explorerRight.reduce((sum: number, entry) => sum + entry.size, 0)
  );

  function openDisk(id: string) {
    const volume = store.volumes.find((item) => item.id === id);
    if (volume) void store.openExplorer(volume.mountPoint);
  }

  function queued(entry: { path: string }): boolean {
    const finding = store.findingForPath(entry.path);
    return finding ? store.isSelected(finding.id) : false;
  }
</script>

<div class="fx">
  <div class="fx-bar">
    <LocationPicker onselect={openDisk} />
    <div class="fx-path" bind:this={pathBar}>
      {#each crumbs as crumb, i (crumb.path)}
        <button
          type="button"
          class="fx-seg"
          class:fx-seg-first={i === 0}
          class:fx-seg-on={i === crumbs.length - 1}
          title={crumb.path}
          onclick={() => void store.explorerAt(crumb.path)}
        >
          {crumb.label}
        </button>
      {/each}
    </div>
    {#if store.explorerSelected}
      <span class="fx-selected">
        {store.explorerSelected.name}
        <span class="fx-selected-size">{formatSize(store.explorerSelected.size)}</span>
      </span>
    {/if}
  </div>

  <div class="fx-main">
    <div class="fx-main-head">
      <div class="fx-main-title">
        <p class="fx-folder-name">{currentName}</p>
        <p class="fx-folder-meta">
          {t('explorer.filesSummary', [
            String(store.explorerRight.length),
            store.explorerTotal === null ? t('explorer.loading') : formatSize(store.explorerTotal)
          ])}
        </p>
      </div>
      <button
        type="button"
        class="fx-ai"
        disabled={store.explorerAi.loading}
        onclick={async () => {
          await store.refreshExplorer(true);
          await store.askExplorerAi();
        }}
      >
        <span class="fx-spin-wrap" class:fx-spin={store.explorerAi.loading}>
          <Icon name={store.explorerAi.loading ? 'refresh' : 'spark'} size={12} />
        </span>
        {store.explorerAi.loading ? t('explorer.aiReading') : t('explorer.aiButton')}
      </button>
    </div>

    {#if store.explorerAi.error}
      <div class="fx-ai-card fx-ai-error">
        <p class="fx-ai-text">{store.explorerAi.error}</p>
      </div>
    {/if}

    <div class="fx-files">
      {#if store.explorerRightLoading}
        <p class="fx-hint">{t('explorer.loading')}</p>
      {:else}
        {#each sortedEntries as entry (entry.path)}
          {@const finding = store.findingForPath(entry.path)}
          {@const isQueued = finding ? store.isSelected(finding.id) : false}
          {@const verdict = store.explorerAi.items[entry.name]}
          <div
            class="fx-file"
            class:fx-file-on={store.explorerSelected?.path === entry.path}
            role="button"
            tabindex="0"
            title={entry.path}
            onclick={() =>
              entry.isDir
                ? void store.enterExplorerFolder(entry)
                : (store.explorerSelected = entry)}
            onkeydown={(event) => {
              if (event.key === 'Enter' || event.key === ' ') {
                event.preventDefault();
                if (entry.isDir) void store.enterExplorerFolder(entry);
                else store.explorerSelected = entry;
              }
            }}
          >
            <Icon name={entry.isDir ? 'folder' : 'file'} size={14} />
            <span class="fx-file-name">{entry.name}</span>
            <span class="fx-file-dir">{entry.isDir ? t('explorer.folderBadge') : ''}</span>
            {#if verdict}
              <span
                class="fx-verdict"
                class:fx-verdict-safe={verdict.verdict === 'safe'}
                class:fx-verdict-keep={verdict.verdict === 'keep'}
                title={verdict.purpose}
              >
                {verdict.verdict === 'safe'
                  ? t('explorer.verdictSafe')
                  : verdict.verdict === 'keep'
                    ? t('explorer.verdictKeep')
                    : t('explorer.verdictCaution')}
              </span>
            {/if}
            <span class="fx-size">{entry.size ? formatSize(entry.size) : '…'}</span>
            <button
              type="button"
              class="fx-act"
              class:fx-act-on={isQueued}
              title={isQueued ? t('sub.remove') : t('sub.add')}
              aria-label={`${isQueued ? t('sub.remove') : t('sub.add')} ${entry.name}`}
              onclick={(event) => {
                event.stopPropagation();
                void store.toggleExplorerCandidate(entry);
              }}
            >
              <Icon name={isQueued ? 'undo' : 'trash'} size={11} />
            </button>
          </div>
        {:else}
          <p class="fx-hint">{t('list.folderEmpty')}</p>
        {/each}
      {/if}
    </div>
  </div>
</div>

<style>
  .fx {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .fx-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
    padding: 10px 14px;
    border-bottom: 1px solid var(--color-border);
  }
  /* Finder-style path bar: interlocking chevron segments. */
  .fx-path {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 0;
    height: 30px;
    padding: 2px;
    border-radius: 8px;
    background: color-mix(in oklch, var(--color-bg) 55%, transparent);
    overflow-x: auto;
    scrollbar-width: none;
  }
  .fx-path::-webkit-scrollbar {
    display: none;
  }
  .fx-seg {
    flex: none;
    height: 26px;
    max-width: 200px;
    padding: 0 11px 0 14px;
    border: none;
    background: var(--color-surface-2);
    color: var(--color-muted);
    font-size: 12px;
    font-weight: 520;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    cursor: pointer;
    clip-path: polygon(
      0 0,
      calc(100% - 7px) 0,
      100% 50%,
      calc(100% - 7px) 100%,
      0 100%,
      7px 50%
    );
  }
  .fx-seg + .fx-seg {
    margin-left: -5px;
  }
  .fx-seg-first {
    padding-left: 10px;
    border-top-left-radius: 6px;
    border-bottom-left-radius: 6px;
    clip-path: polygon(
      0 0,
      calc(100% - 7px) 0,
      100% 50%,
      calc(100% - 7px) 100%,
      0 100%
    );
  }
  .fx-seg:hover {
    background: var(--color-surface-3);
    color: var(--color-fg);
  }
  .fx-seg-on {
    background: color-mix(in oklch, var(--color-violet) 32%, var(--color-surface-2));
    color: var(--color-fg);
    font-weight: 640;
  }
  .fx-selected {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--color-muted);
    white-space: nowrap;
  }
  .fx-selected-size {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--color-faint);
  }
  .fx-main {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .fx-main-head {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--color-border);
  }
  .fx-main-title {
    min-width: 0;
  }
  .fx-folder-name {
    margin: 0;
    font-size: 14px;
    font-weight: 650;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .fx-folder-meta {
    margin: 1px 0 0;
    font-size: 11px;
    color: var(--color-faint);
  }
  .fx-ai {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 11px;
    border-radius: 8px;
    border: 1px solid color-mix(in oklch, var(--color-violet) 50%, transparent);
    background: transparent;
    color: var(--color-violet);
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
  }
  .fx-ai:hover:not(:disabled) {
    background: color-mix(in oklch, var(--color-violet) 10%, transparent);
  }
  .fx-ai:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .fx-spin-wrap {
    display: inline-flex;
  }
  .fx-spin {
    animation: fx-spin 0.9s linear infinite;
  }
  @keyframes fx-spin {
    to {
      transform: rotate(360deg);
    }
  }
  .fx-files {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 6px 8px;
  }
  .fx-file {
    display: flex;
    align-items: center;
    gap: 9px;
    min-height: 36px;
    padding: 0 9px;
    border-radius: 8px;
    color: var(--color-fg);
    cursor: pointer;
  }
  .fx-file:hover {
    background: color-mix(in oklch, var(--color-surface-2) 70%, transparent);
  }
  .fx-file-on {
    background: color-mix(in oklch, var(--color-violet) 22%, transparent);
  }
  .fx-file :global(svg) {
    flex: none;
    color: var(--color-faint);
  }
  .fx-file-name {
    flex: none;
    max-width: 46%;
    font-size: 13.5px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .fx-file-dir {
    flex: 1;
    min-width: 0;
    font-size: 11px;
    color: var(--color-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .fx-verdict {
    flex: none;
    padding: 1px 7px;
    border-radius: 999px;
    border: 1px solid color-mix(in oklch, var(--color-warn) 45%, transparent);
    color: var(--color-warn);
    font-size: 10px;
    font-weight: 600;
    white-space: nowrap;
  }
  .fx-verdict-safe {
    color: var(--color-ok);
    border-color: color-mix(in oklch, var(--color-ok) 45%, transparent);
  }
  .fx-verdict-keep {
    color: var(--color-faint);
    border-color: var(--color-border);
  }
  .fx-size {
    flex: none;
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--color-faint);
  }
  .fx-act {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 6px;
    border: 1px solid var(--color-border);
    background: transparent;
    color: var(--color-faint);
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.12s ease;
  }
  .fx-file:hover .fx-act,
  .fx-act:focus-visible {
    opacity: 1;
  }
  .fx-act-on {
    opacity: 1;
    color: var(--color-violet);
    border-color: color-mix(in oklch, var(--color-violet) 45%, transparent);
  }
  .fx-act:hover {
    color: var(--color-fg);
  }
  .fx-hint {
    margin: 12px 10px;
    font-size: 12px;
    color: var(--color-faint);
  }
  .fx-ai-card {
    flex: none;
    margin: 10px 14px 0;
    padding: 10px 12px;
    border-radius: 10px;
    border: 1px solid color-mix(in oklch, var(--color-warn) 45%, transparent);
    background: color-mix(in oklch, var(--color-warn) 8%, transparent);
  }
  .fx-ai-text {
    margin: 0;
    font-size: 12px;
    line-height: 1.6;
    white-space: pre-wrap;
  }
</style>

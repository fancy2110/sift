<script lang="ts">
  /**
   * Linked two-pane browser: the left lists the current directory's folders,
   * the right shows the children (folders and files) of the selected folder.
   * Works without a scan.
   */
  import Icon from './Icon.svelte';
  import LocationPicker from './LocationPicker.svelte';
  import { store } from '../store.svelte';
  import { formatSize } from '../format';
  import { t } from '../i18n.svelte';
  import { onMount } from 'svelte';

  onMount(() => {
    if (!store.explorerPath) void store.openExplorer();
  });

  const currentName = $derived(
    store.explorerPath.split('/').filter(Boolean).pop() ?? '/'
  );
  const rightName = $derived(store.explorerSelectedDir?.name ?? currentName);
  const crumbs = $derived.by(() => {
    const parts = store.explorerPath.split('/').filter(Boolean);
    const out: { label: string; path: string }[] = [];
    let acc = '';
    for (const part of parts) {
      acc += `/${part}`;
      out.push({ label: part, path: acc });
    }
    return out;
  });
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
    <button
      type="button"
      class="fx-back"
      aria-label={t('sub.goUp')}
      title={t('sub.goUp')}
      onclick={() => store.explorerUp()}
    >
      <Icon name="chevronUp" size={14} />
    </button>
    <div class="fx-crumbs">
      {#each crumbs as crumb, i (crumb.path)}
        {#if i > 0}
          <Icon name="chevronRight" size={10} />
        {/if}
        <button
          type="button"
          class="fx-crumb"
          class:fx-crumb-on={i === crumbs.length - 1}
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

  <div class="fx-body">
    <!-- left: folders of the current directory -->
    <div class="fx-left">
      <div class="fx-col-head" title={store.explorerPath}>{currentName}</div>
      <div class="fx-list">
        {#if store.explorerLeftLoading}
          <p class="fx-hint">{t('explorer.loading')}</p>
        {:else}
          {#each store.explorerLeft as entry (entry.path)}
            <div
              class="fx-row"
              class:fx-row-on={store.explorerSelectedDir?.path === entry.path}
              role="button"
              tabindex="0"
              title={entry.path}
              onclick={() => void store.selectExplorerDir(entry)}
              ondblclick={() => void store.enterExplorerFolder(entry)}
              onkeydown={(event) => {
                if (event.key === 'Enter') {
                  event.preventDefault();
                  void store.enterExplorerFolder(entry);
                } else if (event.key === ' ') {
                  event.preventDefault();
                  void store.selectExplorerDir(entry);
                }
              }}
            >
              <Icon name="folder" size={14} />
              <span class="fx-name">{entry.name}</span>
              <span class="fx-size">{entry.size ? formatSize(entry.size) : ''}</span>
              <button
                type="button"
                class="fx-act"
                class:fx-act-on={queued(entry)}
                title={queued(entry) ? t('sub.remove') : t('sub.add')}
                aria-label={`${queued(entry) ? t('sub.remove') : t('sub.add')} ${entry.name}`}
                onclick={(event) => {
                  event.stopPropagation();
                  void store.toggleExplorerCandidate(entry);
                }}
              >
                <Icon name={queued(entry) ? 'undo' : 'trash'} size={11} />
              </button>
            </div>
          {:else}
            <p class="fx-hint">{t('list.folderEmpty')}</p>
          {/each}
        {/if}
      </div>
    </div>

    <!-- right: children of the selected folder -->
    <div class="fx-right">
      <div class="fx-right-head">
        <div class="fx-right-title">
          <p class="fx-folder-name">{rightName}</p>
          <p class="fx-folder-meta">
            {t('explorer.filesSummary', [
              String(store.explorerRight.length),
              formatSize(totalBytes)
            ])}
          </p>
        </div>
        <button
          type="button"
          class="fx-ai"
          disabled={store.explorerAi.loading}
          onclick={() => void store.askExplorerAi()}
        >
          <span class="fx-spin-wrap" class:fx-spin={store.explorerAi.loading}>
            <Icon name={store.explorerAi.loading ? 'refresh' : 'spark'} size={12} />
          </span>
          {store.explorerAi.loading ? t('explorer.aiReading') : t('explorer.aiButton')}
        </button>
      </div>

      {#if store.explorerAi.text || store.explorerAi.error}
        <div class="fx-ai-card" class:fx-ai-error={Boolean(store.explorerAi.error)}>
          <p class="fx-ai-text">{store.explorerAi.error || store.explorerAi.text}</p>
        </div>
      {/if}

      <div class="fx-files">
        {#if store.explorerRightLoading}
          <p class="fx-hint">{t('explorer.loading')}</p>
        {:else}
          {#each store.explorerRight as entry (entry.path)}
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
              <Icon name={entry.isDir ? 'folder' : 'file'} size={13} />
              <span class="fx-file-name">{entry.name}</span>
              <span class="fx-file-dir">{entry.isDir ? t('explorer.folderBadge') : ''}</span>
              <span class="fx-size">{entry.size ? formatSize(entry.size) : '…'}</span>
              <button
                type="button"
                class="fx-act"
                class:fx-act-on={queued(entry)}
                title={queued(entry) ? t('sub.remove') : t('sub.add')}
                aria-label={`${queued(entry) ? t('sub.remove') : t('sub.add')} ${entry.name}`}
                onclick={(event) => {
                  event.stopPropagation();
                  void store.toggleExplorerCandidate(entry);
                }}
              >
                <Icon name={queued(entry) ? 'undo' : 'trash'} size={11} />
              </button>
            </div>
          {:else}
            <p class="fx-hint">{t('list.folderEmpty')}</p>
          {/each}
        {/if}
      </div>
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
  .fx-back {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    border-radius: 7px;
    border: 1px solid var(--color-border);
    background: transparent;
    color: var(--color-faint);
    cursor: pointer;
  }
  .fx-back:hover {
    color: var(--color-fg);
  }
  .fx-crumbs {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .fx-crumb {
    border: none;
    background: transparent;
    padding: 3px 7px;
    border-radius: 6px;
    font-size: 12.5px;
    color: var(--color-muted);
    white-space: nowrap;
    cursor: pointer;
  }
  .fx-crumb:hover {
    background: color-mix(in oklch, var(--color-surface-2) 70%, transparent);
    color: var(--color-fg);
  }
  .fx-crumb-on {
    color: var(--color-fg);
    font-weight: 620;
  }
  .fx-selected {
    margin-left: auto;
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
  .fx-body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 3fr 7fr;
  }
  .fx-left {
    min-width: 0;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--color-border);
  }
  .fx-col-head {
    flex: none;
    padding: 9px 12px;
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--color-faint);
    border-bottom: 1px solid var(--color-border);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .fx-list,
  .fx-files {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 6px;
  }
  .fx-row,
  .fx-file {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 34px;
    padding: 0 8px;
    border-radius: 7px;
    color: var(--color-fg);
    cursor: pointer;
  }
  .fx-row:hover,
  .fx-file:hover {
    background: color-mix(in oklch, var(--color-surface-2) 70%, transparent);
  }
  .fx-row-on,
  .fx-file-on {
    background: color-mix(in oklch, var(--color-violet) 22%, transparent);
  }
  .fx-row :global(svg),
  .fx-file :global(svg) {
    flex: none;
    color: var(--color-faint);
  }
  .fx-name,
  .fx-file-name {
    font-size: 13.5px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .fx-name {
    flex: 1;
    min-width: 0;
  }
  .fx-file-name {
    flex: none;
    max-width: 55%;
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
  .fx-row:hover .fx-act,
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
  .fx-right {
    min-width: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .fx-right-head {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 14px;
    border-bottom: 1px solid var(--color-border);
  }
  .fx-right-title {
    min-width: 0;
  }
  .fx-folder-name {
    margin: 0;
    font-size: 13.5px;
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
  .fx-ai-card {
    flex: none;
    margin: 10px 14px 0;
    padding: 10px 12px;
    border-radius: 10px;
    border: 1px solid color-mix(in oklch, var(--color-violet) 30%, var(--color-border));
    background: color-mix(in oklch, var(--color-violet) 7%, transparent);
  }
  .fx-ai-error {
    border-color: color-mix(in oklch, var(--color-warn) 45%, transparent);
    background: color-mix(in oklch, var(--color-warn) 8%, transparent);
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
  .fx-ai-text {
    margin: 0;
    font-size: 12px;
    line-height: 1.6;
    white-space: pre-wrap;
    color: var(--color-fg);
  }
</style>

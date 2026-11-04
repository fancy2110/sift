<script lang="ts">
  /**
   * Finder-style column browser. Works without a completed scan: each column
   * lists one directory on demand and fills in child-folder totals lazily.
   */
  import Icon from './Icon.svelte';
  import LocationPicker from './LocationPicker.svelte';
  import { store } from '../store.svelte';
  import { formatSize } from '../format';
  import { t } from '../i18n.svelte';
  import { onMount } from 'svelte';

  onMount(() => {
    if (store.explorerColumns.length === 0) void store.openExplorer();
  });

  function openDisk(id: string) {
    const volume = store.volumes.find((item) => item.id === id);
    if (volume) void store.openExplorer(volume.mountPoint);
  }

  const crumbs = $derived.by(() => {
    const path = store.explorerColumns.at(-1)?.path ?? '';
    const parts = path.split('/').filter(Boolean);
    const out: { label: string; path: string }[] = [];
    let acc = '';
    for (const part of parts) {
      acc += `/${part}`;
      out.push({ label: part, path: acc });
    }
    return out;
  });
</script>

<div class="fx">
  <div class="fx-bar">
    <LocationPicker onselect={openDisk} />
    <button
      type="button"
      class="fx-back"
      disabled={store.explorerColumns.length <= 1}
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

  <div class="fx-columns">
    {#each store.explorerColumns as column, ci (column.path + ci)}
      <div class="fx-col">
        <div class="fx-col-head" title={column.path}>{column.name}</div>
        <div class="fx-col-body">
          {#if column.loading}
            <p class="fx-hint">{t('explorer.loading')}</p>
          {:else}
            {#each column.entries as entry (entry.path)}
              {@const finding = store.findingForPath(entry.path)}
              {@const queued = finding ? store.isSelected(finding.id) : false}
              <div
                class="fx-row"
                class:fx-row-on={store.explorerSelected?.path === entry.path}
                role="button"
                tabindex="0"
                title={entry.path}
                onclick={() => void store.selectExplorerEntry(ci, entry)}
                onkeydown={(event) => {
                  if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    void store.selectExplorerEntry(ci, entry);
                  }
                }}
                oncontextmenu={(event) => {
                  event.preventDefault();
                  void store.toggleExplorerCandidate(entry);
                }}
              >
                <Icon name={entry.isDir ? 'folder' : 'file'} size={14} />
                <span class="fx-name">{entry.name}</span>
                <span class="fx-size">
                  {entry.isDir && !column.sized ? '…' : formatSize(entry.size)}
                </span>
                <button
                  type="button"
                  class="fx-act"
                  class:fx-act-on={Boolean(queued)}
                  title={queued ? t('sub.remove') : t('sub.add')}
                  aria-label={`${queued ? t('sub.remove') : t('sub.add')} ${entry.name}`}
                  onclick={(event) => {
                    event.stopPropagation();
                    void store.toggleExplorerCandidate(entry);
                  }}
                >
                  <Icon name={queued ? 'undo' : 'trash'} size={11} />
                </button>
              </div>
            {:else}
              <p class="fx-hint">{t('list.folderEmpty')}</p>
            {/each}
          {/if}
        </div>
      </div>
    {/each}
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
  .fx-back:hover:not(:disabled) {
    color: var(--color-fg);
  }
  .fx-back:disabled {
    opacity: 0.4;
    cursor: default;
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
  .fx-columns {
    flex: 1;
    min-height: 0;
    display: flex;
    overflow-x: auto;
  }
  .fx-col {
    flex: none;
    width: 250px;
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--color-border);
  }
  .fx-col:last-child {
    border-right: none;
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
  .fx-col-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 4px;
  }
  .fx-row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 34px;
    padding: 0 8px;
    border: none;
    border-radius: 7px;
    background: transparent;
    color: var(--color-fg);
    text-align: left;
    cursor: pointer;
  }
  .fx-row:hover {
    background: color-mix(in oklch, var(--color-surface-2) 70%, transparent);
  }
  .fx-row-on {
    background: color-mix(in oklch, var(--color-violet) 22%, transparent);
  }
  .fx-row :global(svg) {
    flex: none;
    color: var(--color-faint);
  }
  .fx-name {
    flex: 1;
    min-width: 0;
    font-size: 13.5px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
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
  .fx-size {
    flex: none;
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--color-faint);
  }
  .fx-hint {
    margin: 10px 8px;
    font-size: 12px;
    color: var(--color-faint);
  }
</style>

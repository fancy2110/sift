<script lang="ts">
  import { onMount } from 'svelte';
  import type { AppUninstallPlan, UninstallManifest } from '../types';
  import {
    executeAppUninstall,
    planAppUninstall,
    restoreAppUninstall
  } from '../ipc';
  import { t } from '../i18n.svelte';
  import { formatSize } from '../format';
  import Icon from './Icon.svelte';

  let {
    appKey,
    appName,
    onClose
  }: { appKey: string; appName: string; onClose: () => void } = $props();

  let plan = $state<AppUninstallPlan | null>(null);
  let selected = $state<Set<string>>(new Set());
  let loading = $state(true);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let manifest = $state<UninstallManifest | null>(null);
  let restoreMessage = $state<string | null>(null);

  onMount(load);

  async function load() {
    loading = true;
    error = null;
    try {
      plan = await planAppUninstall(appKey);
      selected = new Set(
        plan.artifacts
          .filter((artifact) => artifact.disposition === 'autoSafe' && artifact.exists)
          .map((artifact) => artifact.path)
      );
    } catch (cause) {
      error = String(cause);
    } finally {
      loading = false;
    }
  }

  function selectable(artifact: { disposition: string; exists: boolean }) {
    return artifact.exists && artifact.disposition !== 'keep' && artifact.disposition !== 'forbidden';
  }

  function toggle(path: string) {
    const next = new Set(selected);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    selected = next;
  }

  async function execute() {
    if (!plan || selected.size === 0) return;
    if (!confirm(t('uninstall.confirm'))) return;
    busy = true;
    error = null;
    try {
      manifest = await executeAppUninstall(appKey, [...selected]);
      restoreMessage = null;
    } catch (cause) {
      error = String(cause);
    } finally {
      busy = false;
    }
  }

  async function restore() {
    if (!manifest) return;
    busy = true;
    error = null;
    try {
      const report = await restoreAppUninstall(manifest.id);
      restoreMessage = t('uninstall.restored', [report.restored.length]);
    } catch (cause) {
      error = String(cause);
    } finally {
      busy = false;
    }
  }

  const selectedBytes = $derived(
    plan
      ? plan.artifacts
          .filter((artifact) => selected.has(artifact.path))
          .reduce((sum, artifact) => sum + (artifact.sizeBytes ?? 0), 0)
      : 0
  );
</script>

<div class="uninstall-backdrop" role="presentation">
  <div
    class="uninstall-panel"
    role="dialog"
    aria-modal="true"
    aria-label={t('uninstall.title', [appName])}
  >
    <header class="uninstall-head">
      <div>
        <h2>{t('uninstall.title', [appName])}</h2>
        <p>{t('uninstall.planHint')}</p>
      </div>
      <button class="icon-btn" type="button" aria-label={t('common.close')} onclick={onClose}>
        <Icon name="x" size={16} />
      </button>
    </header>

    {#if loading}
      <p class="uninstall-empty">{t('uninstall.loading')}</p>
    {:else if error}
      <p class="uninstall-error">{error}</p>
    {:else if plan}
      {#if manifest}
        <div class="uninstall-done">
          <Icon name="check" size={18} />
          <p>{t('uninstall.done', [manifest.entries.length, formatSize(plan.totalBytes)])}</p>
        </div>
        {#if restoreMessage}
          <p class="uninstall-note">{restoreMessage}</p>
        {/if}
        <footer class="uninstall-foot">
          <button class="btn btn-quiet" type="button" onclick={restore} disabled={busy}>
            {t('uninstall.restore')}
          </button>
          <button class="btn btn-primary" type="button" onclick={onClose}>
            {t('uninstall.close')}
          </button>
        </footer>
      {:else}
        <div class="uninstall-list">
          {#each plan.artifacts as artifact (artifact.path)}
            <label class="uninstall-row" class:uninstall-row-off={!selectable(artifact)}>
              <input
                type="checkbox"
                checked={selected.has(artifact.path)}
                disabled={!selectable(artifact)}
                onchange={() => toggle(artifact.path)}
              />
              <span class="uninstall-kind">{artifact.kind}</span>
              <span class="uninstall-path" title={artifact.path}>{artifact.path}</span>
              {#if artifact.sharedWith?.length}
                <span class="uninstall-shared">
                  {t('uninstall.shared', [artifact.sharedWith.join(', ')])}
                </span>
              {/if}
              <span class="num uninstall-size">
                {artifact.sizeBytes === null ? '—' : formatSize(artifact.sizeBytes)}
              </span>
            </label>
          {:else}
            <p class="uninstall-empty">{t('uninstall.empty')}</p>
          {/each}
        </div>
        <footer class="uninstall-foot">
          <span class="num uninstall-total">{formatSize(selectedBytes)}</span>
          <button class="btn btn-quiet" type="button" onclick={onClose} disabled={busy}>
            {t('uninstall.cancel')}
          </button>
          <button
            class="btn btn-danger"
            type="button"
            onclick={execute}
            disabled={busy || selected.size === 0}
          >
            <Icon name="trash" size={14} />
            {t('uninstall.execute')}
          </button>
        </footer>
      {/if}
    {/if}
  </div>
</div>

<style>
  .uninstall-backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: grid;
    place-items: center;
    padding: 24px;
    background: color-mix(in oklch, var(--color-bg) 55%, transparent);
    backdrop-filter: blur(6px);
  }
  .uninstall-panel {
    width: min(720px, 100%);
    max-height: min(80vh, 720px);
    display: flex;
    flex-direction: column;
    border-radius: 18px;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
    box-shadow: 0 32px 80px -30px var(--color-shadow);
    overflow: hidden;
  }
  .uninstall-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    padding: 20px 22px 14px;
    border-bottom: 1px solid var(--color-border);
  }
  .uninstall-head h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 650;
  }
  .uninstall-head p {
    margin: 4px 0 0;
    font-size: 12px;
    color: var(--color-faint);
  }
  .icon-btn {
    border: none;
    background: transparent;
    color: var(--color-muted);
    cursor: pointer;
    padding: 6px;
    border-radius: 8px;
  }
  .uninstall-list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 8px 14px;
  }
  .uninstall-row {
    display: grid;
    grid-template-columns: 18px 110px minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 10px;
    padding: 8px 8px;
    border-bottom: 1px solid color-mix(in oklch, var(--color-border) 55%, transparent);
    font-size: 12.5px;
    cursor: pointer;
  }
  .uninstall-row-off {
    opacity: 0.45;
    cursor: default;
  }
  .uninstall-kind {
    color: var(--color-faint);
    font-size: 11px;
  }
  .uninstall-path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .uninstall-shared {
    color: var(--color-warn);
    font-size: 11px;
  }
  .uninstall-size {
    color: var(--color-muted);
  }
  .uninstall-foot {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 10px;
    padding: 14px 22px;
    border-top: 1px solid var(--color-border);
  }
  .uninstall-total {
    margin-right: auto;
    color: var(--color-faint);
    font-size: 12px;
  }
  .uninstall-empty,
  .uninstall-error,
  .uninstall-note {
    padding: 24px 22px;
    font-size: 12.5px;
    color: var(--color-faint);
  }
  .uninstall-error {
    color: var(--color-danger);
  }
  .uninstall-done {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 28px 22px 8px;
    color: var(--color-ok);
  }
  .uninstall-done p {
    margin: 0;
    color: var(--color-fg);
    font-size: 13px;
  }
</style>

<script lang="ts">
  /**
   * User-defined "directory + file-name regex" cleanup rules.
   *
   * The list is for reading and deleting only; creating a rule (and trying it
   * out) happens in a modal dialog, which keeps every row to a single line.
   */
  import Icon from './Icon.svelte';
  import { store } from '../store.svelte';
  import { formatSize } from '../format';
  import { t } from '../i18n.svelte';
  import type { CleanupRule, RulePreview } from '../ipc';

  let dialogOpen = $state(false);
  let draft = $state<CleanupRule | null>(null);
  let preview = $state<RulePreview | null>(null);
  let busy = $state(false);

  function openDialog() {
    draft = {
      id: '',
      name: '',
      dir: '',
      pattern: '',
      olderThanDays: 0,
      enabled: true
    };
    preview = null;
    busy = false;
    dialogOpen = true;
  }

  function closeDialog() {
    if (busy) return;
    dialogOpen = false;
    draft = null;
    preview = null;
  }

  async function previewRule() {
    if (!draft || busy) return;
    busy = true;
    preview = await store.previewCleanupRule($state.snapshot(draft));
    busy = false;
  }

  async function saveRule() {
    if (!draft || busy) return;
    busy = true;
    const saved = await store.saveCleanupRuleDraft($state.snapshot(draft));
    busy = false;
    if (saved) closeDialog();
  }

  /**
   * The routines tab lives inside a horizontally translated pager, so a
   * `position: fixed` child would be centered on that 400%-wide panel
   * instead of the viewport. Render the modal on <body> instead.
   */
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return {
      destroy() {
        node.remove();
      }
    };
  }

  const ageText = (rule: CleanupRule) =>
    rule.olderThanDays > 0
      ? t('settings.ruleDays', [String(rule.olderThanDays)])
      : t('settings.ruleAnyAge');
</script>

<svelte:window
  onkeydown={(event) => {
    if (dialogOpen && event.key === 'Escape') closeDialog();
  }}
/>

<div class="cr">
  <div class="cr-head">
    <p class="cr-desc">{t('settings.rulesDesc')}</p>
    <button type="button" class="cr-add" onclick={openDialog}>
      <Icon name="play" size={10} /> {t('settings.ruleAdd')}
    </button>
  </div>

  {#each store.cleanupRules as rule (rule.id)}
    <div class="cr-row" class:cr-off={!rule.enabled}>
      <span class="cr-name" title={rule.name}>{rule.name}</span>
      <span class="cr-meta" title={`${rule.dir} · ${rule.pattern}`}>
        {rule.dir} · <code>{rule.pattern}</code> · {ageText(rule)}
      </span>
      <button
        type="button"
        class="cr-del"
        title={t('settings.delete')}
        aria-label={`${t('settings.delete')} ${rule.name}`}
        onclick={() => store.removeCleanupRule(rule.id)}
      >
        <Icon name="trash" size={13} />
      </button>
    </div>
  {:else}
    <p class="cr-empty">{t('settings.rulesEmpty')}</p>
  {/each}
</div>

{#if dialogOpen && draft}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="cr-overlay" role="presentation" use:portal onclick={closeDialog}>
    <div
      class="cr-dialog"
      role="dialog"
      tabindex="-1"
      aria-modal="true"
      aria-label={t('settings.ruleAdd')}
      onclick={(event) => event.stopPropagation()}
    >
      <div class="cr-dialog-head">
        <h3 class="cr-dialog-title">{t('settings.ruleAdd')}</h3>
        <button
          type="button"
          class="cr-icon-btn"
          aria-label={t('settings.ruleCancel')}
          onclick={closeDialog}
        >
          <Icon name="x" size={13} />
        </button>
      </div>

      <label class="cr-field">
        <span>{t('settings.ruleName')}</span>
        <input
          class="cr-input"
          placeholder={t('settings.ruleNamePh')}
          bind:value={draft.name}
        />
      </label>

      <label class="cr-field">
        <span>{t('settings.ruleDir')}</span>
        <input
          class="cr-input"
          placeholder={t('settings.ruleDirPh')}
          spellcheck="false"
          bind:value={draft.dir}
        />
      </label>

      <label class="cr-field">
        <span>{t('settings.rulePattern')}</span>
        <input
          class="cr-input"
          placeholder={t('settings.rulePatternPh')}
          spellcheck="false"
          bind:value={draft.pattern}
        />
      </label>

      <label class="cr-field cr-field-inline">
        <span>{t('settings.ruleDaysLabel')}</span>
        <input type="number" min="0" max="3650" bind:value={draft.olderThanDays} />
      </label>

      <label class="cr-enabled">
        <input type="checkbox" bind:checked={draft.enabled} />
        <span>{t('routines.stateOn')}</span>
      </label>

      {#if preview}
        <p class="cr-preview">
          {preview.matched === 0
            ? t('settings.rulePreviewEmpty')
            : t('settings.rulePreviewResult', [
                String(preview.matched),
                formatSize(preview.bytes)
              ])}
          {#if preview.truncated}
            · {t('settings.ruleTruncated')}
          {/if}
        </p>
      {/if}

      <div class="cr-dialog-actions">
        <button type="button" class="cr-btn-text" disabled={busy} onclick={previewRule}>
          {t('settings.rulePreview')}
        </button>
        <span class="cr-spacer"></span>
        <button type="button" class="cr-btn-text" onclick={closeDialog}>
          {t('settings.ruleCancel')}
        </button>
        <button
          type="button"
          class="cr-btn-text cr-primary"
          disabled={busy}
          onclick={saveRule}
        >
          {t('settings.ruleSave')}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .cr {
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: 100%;
    min-width: 0;
  }
  .cr-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }
  .cr-desc {
    margin: 0;
    font-size: 11px;
    line-height: 1.5;
    color: var(--color-faint);
  }
  .cr-empty {
    margin: 0;
    padding: 4px 0;
    font-size: 11.5px;
    color: var(--color-faint);
  }

  /* one rule == one line */
  .cr-row {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
  }
  .cr-row.cr-off {
    opacity: 0.55;
  }
  .cr-name {
    flex: none;
    max-width: 34%;
    font-size: 12.5px;
    font-weight: 620;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cr-meta {
    flex: 1;
    min-width: 0;
    font-size: 11px;
    color: var(--color-faint);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cr-meta code {
    font-family: var(--font-mono);
  }
  .cr-del {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    border-radius: 8px;
    border: 1px solid transparent;
    background: transparent;
    color: var(--color-faint);
    cursor: pointer;
  }
  .cr-del:hover {
    color: var(--color-warn);
    border-color: var(--color-border);
  }

  .cr-add {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 11px;
    border-radius: 8px;
    border: 1px dashed var(--color-border-strong);
    background: transparent;
    color: var(--color-muted);
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
  }
  .cr-add:hover {
    color: var(--color-fg);
    border-color: var(--color-violet);
  }

  /* dialog */
  .cr-overlay {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
    background: oklch(0 0 0 / 0.42);
    backdrop-filter: blur(3px);
    -webkit-backdrop-filter: blur(3px);
  }
  .cr-dialog {
    width: min(440px, calc(100vw - 40px));
    display: flex;
    flex-direction: column;
    gap: 11px;
    padding: 18px 20px 16px;
    border-radius: 16px;
    border: 1px solid var(--color-border-strong);
    background: color-mix(in oklch, var(--color-surface) 96%, var(--color-bg));
    box-shadow: 0 30px 70px -24px var(--color-shadow);
  }
  .cr-dialog-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .cr-dialog-title {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .cr-icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    border-radius: 8px;
    border: none;
    background: transparent;
    color: var(--color-faint);
    cursor: pointer;
  }
  .cr-icon-btn:hover {
    color: var(--color-fg);
    background: var(--color-surface-2);
  }
  .cr-field {
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-size: 10.5px;
    font-weight: 600;
    color: var(--color-faint);
  }
  .cr-field-inline {
    flex-direction: row;
    align-items: center;
    justify-content: space-between;
  }
  .cr-input {
    width: 100%;
    height: 32px;
    padding: 0 10px;
    border-radius: 9px;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
    color: var(--color-fg);
    font-size: 12px;
    outline: none;
  }
  .cr-input:focus {
    border-color: var(--color-violet);
    box-shadow: 0 0 0 3px color-mix(in oklch, var(--color-violet) 16%, transparent);
  }
  .cr-field-inline input {
    width: 76px;
    height: 30px;
    padding: 0 8px;
    border-radius: 8px;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
    color: var(--color-fg);
    font-family: var(--font-mono);
    font-size: 11.5px;
    outline: none;
  }
  .cr-enabled {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 11.5px;
    color: var(--color-muted);
  }
  .cr-enabled input {
    accent-color: var(--color-violet);
  }
  .cr-preview {
    margin: 0;
    font-size: 11px;
    color: var(--color-muted);
  }
  .cr-dialog-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 2px;
  }
  .cr-spacer {
    flex: 1;
  }
  .cr-btn-text {
    padding: 7px 12px;
    border-radius: 8px;
    border: 1px solid var(--color-border);
    background: transparent;
    color: var(--color-muted);
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
  }
  .cr-btn-text:hover {
    color: var(--color-fg);
  }
  .cr-btn-text:disabled {
    opacity: 0.55;
  }
  .cr-primary {
    border-color: color-mix(in oklch, var(--color-violet) 55%, transparent);
    color: var(--color-violet);
  }
</style>

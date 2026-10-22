<script lang="ts">
  /**
   * User-defined "directory + file-name regex" cleanup rules.
   *
   * Rendered both in the settings sheet and in the routines tab, so it keeps
   * its own styles and only depends on the shared store actions.
   */
  import Icon from './Icon.svelte';
  import { store } from '../store.svelte';
  import { formatSize } from '../format';
  import { t } from '../i18n.svelte';
  import type { CleanupRule, RulePreview } from '../ipc';

  let draft = $state<CleanupRule | null>(null);
  let preview = $state<RulePreview | null>(null);
  let busy = $state(false);

  function startDraft() {
    draft = {
      id: '',
      name: '',
      dir: '',
      pattern: '',
      olderThanDays: 0,
      enabled: true
    };
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
    if (saved) {
      draft = null;
      preview = null;
    }
  }

  async function runRule(rule: CleanupRule) {
    if (!window.confirm(t('settings.ruleRunConfirm'))) return;
    await store.runCleanupRule(rule);
  }

  const ageText = (rule: CleanupRule) =>
    rule.olderThanDays > 0
      ? t('settings.ruleDays', [String(rule.olderThanDays)])
      : t('settings.ruleAnyAge');
</script>

<div class="cr">
  <p class="cr-desc">{t('settings.rulesDesc')}</p>

  {#each store.cleanupRules as rule (rule.id)}
    <div class="cr-row">
      <div class="cr-info">
        <p class="cr-name">{rule.name}</p>
        <p class="cr-meta" title={rule.dir}>{rule.dir}</p>
        <p class="cr-meta"><code>{rule.pattern}</code> · {ageText(rule)}</p>
      </div>
      <button
        type="button"
        class="cr-btn"
        title={t('settings.ruleRun')}
        aria-label={t('settings.ruleRun')}
        onclick={() => runRule(rule)}
      >
        <Icon name="play" size={12} />
      </button>
      <button
        type="button"
        class="cr-toggle"
        class:cr-on={rule.enabled}
        aria-pressed={rule.enabled}
        title={rule.enabled ? t('routines.stateOn') : t('routines.stateOff')}
        aria-label={rule.enabled ? t('routines.stateOn') : t('routines.stateOff')}
        onclick={() => store.toggleCleanupRule(rule)}
      >
        <span class="cr-knob"></span>
      </button>
      <button
        type="button"
        class="cr-btn cr-danger"
        title={t('settings.delete')}
        aria-label={t('settings.delete')}
        onclick={() => store.removeCleanupRule(rule.id)}
      >
        <Icon name="x" size={12} />
      </button>
    </div>
  {/each}

  {#if store.cleanupRules.length === 0 && !draft}
    <p class="cr-empty">{t('settings.rulesEmpty')}</p>
  {/if}

  {#if draft}
    <div class="cr-form">
      <input
        class="cr-input"
        placeholder={t('settings.ruleNamePh')}
        aria-label={t('settings.ruleName')}
        bind:value={draft.name}
      />
      <input
        class="cr-input"
        placeholder={t('settings.ruleDirPh')}
        aria-label={t('settings.ruleDir')}
        spellcheck="false"
        bind:value={draft.dir}
      />
      <input
        class="cr-input"
        placeholder={t('settings.rulePatternPh')}
        aria-label={t('settings.rulePattern')}
        spellcheck="false"
        bind:value={draft.pattern}
      />
      <label class="cr-days">
        <span>{t('settings.ruleDaysLabel')}</span>
        <input type="number" min="0" max="3650" bind:value={draft.olderThanDays} />
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
      <div class="cr-actions">
        <button type="button" class="cr-btn-text" disabled={busy} onclick={previewRule}>
          {t('settings.rulePreview')}
        </button>
        <button type="button" class="cr-btn-text cr-primary" disabled={busy} onclick={saveRule}>
          {t('settings.ruleSave')}
        </button>
        <button type="button" class="cr-btn-text" onclick={() => (draft = null)}>
          {t('settings.ruleCancel')}
        </button>
      </div>
    </div>
  {:else}
    <button type="button" class="cr-add" onclick={startDraft}>
      {t('settings.ruleAdd')}
    </button>
  {/if}
</div>

<style>
  .cr {
    display: flex;
    flex-direction: column;
    gap: 9px;
    width: 100%;
    min-width: 0;
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
  .cr-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 9px 10px;
    border-radius: 10px;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
  }
  .cr-info {
    flex: 1;
    min-width: 0;
  }
  .cr-name {
    margin: 0;
    font-size: 12.5px;
    font-weight: 620;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cr-meta {
    margin: 1px 0 0;
    font-size: 10.5px;
    color: var(--color-faint);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cr-meta code {
    font-family: var(--font-mono);
  }
  .cr-btn {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    border-radius: 8px;
    border: 1px solid var(--color-border);
    background: transparent;
    color: var(--color-faint);
    cursor: pointer;
  }
  .cr-btn:hover {
    color: var(--color-violet);
    border-color: var(--color-border-strong);
  }
  .cr-danger:hover {
    color: var(--color-warn);
  }
  .cr-toggle {
    position: relative;
    flex: none;
    width: 34px;
    height: 20px;
    border-radius: 999px;
    border: none;
    background: var(--color-surface-3);
    cursor: pointer;
    transition: background 0.2s ease;
  }
  .cr-toggle.cr-on {
    background: var(--color-violet);
  }
  .cr-knob {
    position: absolute;
    top: 2.5px;
    left: 2.5px;
    width: 15px;
    height: 15px;
    border-radius: 50%;
    background: var(--color-sheen);
    transition: transform 0.2s cubic-bezier(0.4, 0, 0.2, 1);
  }
  .cr-toggle.cr-on .cr-knob {
    transform: translateX(14px);
  }
  .cr-form {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .cr-input {
    width: 100%;
    height: 30px;
    padding: 0 10px;
    border-radius: 9px;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
    color: var(--color-fg);
    font-size: 11.5px;
    outline: none;
  }
  .cr-input:focus {
    border-color: var(--color-violet);
    box-shadow: 0 0 0 3px color-mix(in oklch, var(--color-violet) 16%, transparent);
  }
  .cr-days {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    font-size: 11px;
    color: var(--color-faint);
  }
  .cr-days input {
    width: 72px;
    height: 28px;
    padding: 0 8px;
    border-radius: 8px;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
    color: var(--color-fg);
    font-family: var(--font-mono);
    font-size: 11.5px;
    outline: none;
  }
  .cr-preview {
    margin: 0;
    font-size: 11px;
    color: var(--color-muted);
  }
  .cr-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .cr-btn-text {
    padding: 6px 11px;
    border-radius: 8px;
    border: 1px solid var(--color-border);
    background: transparent;
    color: var(--color-muted);
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
  }
  .cr-btn-text:hover {
    color: var(--color-fg);
  }
  .cr-primary {
    border-color: color-mix(in oklch, var(--color-violet) 55%, transparent);
    color: var(--color-violet);
  }
  .cr-add {
    align-self: flex-start;
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
</style>

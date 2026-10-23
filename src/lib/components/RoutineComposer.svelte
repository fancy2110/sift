<script lang="ts">
  /**
   * "Add routine" dialog with two entry modes: manual configuration and an AI
   * draft. Both end in the same confirm step and always create an
   * approve-before-run routine.
   */
  import Icon from './Icon.svelte';
  import { store } from '../store.svelte';
  import { t } from '../i18n.svelte';
  import { pickDirectory } from '../dialog';
  import type { AiRoutineDraft } from '../types';

  let open = $state(false);
  let mode = $state<'manual' | 'ai'>('manual');
  let busy = $state(false);

  let title = $state('');
  let cadence = $state('routine.cadence.weekly');
  let pathInput = $state('');
  let manualPaths = $state<string[]>([]);

  // Optional rule filter on top of the same form.
  let pattern = $state('');
  let olderThanDays = $state(0);

  let prompt = $state('');
  let draft = $state<AiRoutineDraft | null>(null);

  /** The routines tab lives in a transformed pager; mount the dialog on body. */
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return {
      destroy() {
        node.remove();
      }
    };
  }

  /** Accept legacy `cadence.weekly` values as well as `routine.cadence.*`. */
  function cadenceLabel(value: string) {
    return t(value.startsWith('routine.') ? value : `routine.${value}`);
  }

  function openDialog() {
    mode = 'manual';
    busy = false;
    title = '';
    cadence = 'routine.cadence.weekly';
    pathInput = '';
    manualPaths = [];
    pattern = '';
    olderThanDays = 0;
    prompt = '';
    draft = null;
    open = true;
  }

  function close() {
    if (busy) return;
    open = false;
    draft = null;
  }

  function addPath() {
    const value = pathInput.trim();
    if (!value) return;
    if (!manualPaths.includes(value)) manualPaths = [...manualPaths, value];
    pathInput = '';
  }

  async function choosePath() {
    const selected = await pickDirectory('');
    if (selected && !manualPaths.includes(selected)) {
      manualPaths = [...manualPaths, selected];
    }
  }

  /** With a pattern the entry becomes a rule (daily, directory + regex);
   *  without one it is a path routine with the chosen cadence. */
  async function persist(
    titleValue: string,
    cadenceValue: string,
    paths: string[],
    patternValue = pattern,
    daysValue = olderThanDays
  ) {
    if (patternValue.trim()) {
      return store.saveCleanupRuleDraft({
        id: '',
        name: titleValue,
        dir: paths[0],
        pattern: patternValue.trim(),
        olderThanDays: daysValue,
        enabled: true
      });
    }
    return store.saveRoutineDraft({ title: titleValue, cadence: cadenceValue, paths });
  }

  async function saveManual() {
    if (busy || !title.trim() || manualPaths.length === 0) return;
    busy = true;
    const saved = await persist(title.trim(), cadence, manualPaths);
    busy = false;
    if (saved) open = false;
  }

  async function generate() {
    const text = prompt.trim();
    if (!text || busy) return;
    busy = true;
    draft = await store.generateAiRoutine(text);
    busy = false;
  }

  async function saveDraft() {
    if (!draft || busy) return;
    busy = true;
    const saved = await persist(
      draft.title,
      draft.cadence,
      draft.paths,
      draft.pattern,
      draft.olderThanDays
    );
    busy = false;
    if (saved) open = false;
  }
</script>

<button type="button" class="rc-trigger" onclick={openDialog}>
  <Icon name="play" size={10} /> {t('routines.addTask')}
</button>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="rc-overlay" role="presentation" use:portal onclick={close}>
    <div
      class="rc-dialog"
      role="dialog"
      tabindex="-1"
      aria-modal="true"
      aria-label={t('routines.addTask')}
      onclick={(event) => event.stopPropagation()}
    >
      <div class="rc-head">
        <h3 class="rc-title">{t('routines.addTask')}</h3>
        <button type="button" class="rc-icon" aria-label={t('settings.ruleCancel')} onclick={close}>
          <Icon name="x" size={13} />
        </button>
      </div>

      <div class="rc-tabs" role="tablist">
        <button
          type="button"
          role="tab"
          aria-selected={mode === 'manual'}
          class="rc-tab"
          class:rc-tab-on={mode === 'manual'}
          onclick={() => (mode = 'manual')}
        >
          {t('routines.manualTab')}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={mode === 'ai'}
          class="rc-tab"
          class:rc-tab-on={mode === 'ai'}
          onclick={() => (mode = 'ai')}
        >
          <Icon name="spark" size={11} />
          {t('routines.aiTab')}
        </button>
      </div>

      {#if mode === 'manual'}
        <label class="rc-field">
          <span>{t('routines.titleLabel')}</span>
          <input class="rc-input" bind:value={title} placeholder={t('routines.titlePh')} />
        </label>

        <label class="rc-field">
          <span>{t('routines.cadenceLabel')}</span>
          <select class="rc-input" bind:value={cadence}>
            <option value="routine.cadence.daily">{t('routine.cadence.daily')}</option>
            <option value="routine.cadence.weekly">{t('routine.cadence.weekly')}</option>
            <option value="routine.cadence.monthly">{t('routine.cadence.monthly')}</option>
          </select>
        </label>

        <label class="rc-field">
          <span>{t('settings.rulePattern')} — {t('routines.optional')}</span>
          <input class="rc-input" bind:value={pattern} placeholder={t('settings.rulePatternPh')} spellcheck="false" />
        </label>
        <label class="rc-field rc-field-inline">
          <span>{t('settings.ruleDaysLabel')}</span>
          <input type="number" min="0" max="3650" bind:value={olderThanDays} />
        </label>

        <p class="rc-label">{t('routines.pathsLabel')}</p>
        <div class="rc-paths">
          {#each manualPaths as path (path)}
            <span class="rc-chip" title={path}>
              <span class="rc-chip-text">{path}</span>
              <button
                type="button"
                aria-label={`${t('routines.removePath')} ${path}`}
                onclick={() => (manualPaths = manualPaths.filter((item) => item !== path))}
              >
                <Icon name="x" size={9} />
              </button>
            </span>
          {:else}
            <p class="rc-empty">{t('routines.pathEmpty')}</p>
          {/each}
        </div>

        <div class="rc-path-row">
          <input
            class="rc-input"
            bind:value={pathInput}
            placeholder={t('routines.pathPh')}
            spellcheck="false"
            onkeydown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                addPath();
              }
            }}
          />
          <button type="button" class="rc-btn" onclick={addPath}>
            {t('routines.addPath')}
          </button>
          <button type="button" class="rc-btn" onclick={choosePath}>
            {t('routines.pickPath')}
          </button>
        </div>

        <div class="rc-actions">
          <button type="button" class="rc-btn" onclick={close}>
            {t('settings.ruleCancel')}
          </button>
          <button
            type="button"
            class="rc-btn rc-primary"
            disabled={busy || !title.trim() || manualPaths.length === 0}
            onclick={saveManual}
          >
            {t('routines.saveTask')}
          </button>
        </div>
      {:else}
        <p class="rc-desc">{t('routines.aiDesc')}</p>
        <textarea
          class="rc-input rc-textarea"
          rows="3"
          bind:value={prompt}
          placeholder={t('routines.aiPlaceholder')}
          onkeydown={(event) => {
            if (event.key === 'Enter' && !event.shiftKey) {
              event.preventDefault();
              void generate();
            }
          }}
        ></textarea>
        <div class="rc-actions">
          <button
            type="button"
            class="rc-btn rc-primary"
            disabled={busy || !prompt.trim()}
            onclick={generate}
          >
            <Icon name="spark" size={11} />
            {busy ? t('prompt.thinking') : t('routines.aiGenerate')}
          </button>
        </div>

        {#if draft}
          <div class="rc-draft">
            <p class="rc-draft-title">
              {draft.title}
              <span class="rc-chip-soft">{cadenceLabel(draft.cadence)}</span>
            </p>
            {#each draft.paths as path (path)}
              <p class="rc-draft-path" title={path}>{path}</p>
            {/each}
            {#if draft.pattern}
              <p class="rc-draft-path" title={draft.pattern}>
                <code>{draft.pattern}</code>
                {#if draft.olderThanDays > 0}
                  · {t('settings.ruleDays', [String(draft.olderThanDays)])}
                {/if}
              </p>
            {/if}
            {#if draft.reason}
              <p class="rc-draft-reason">{draft.reason}</p>
            {/if}
          </div>
          <div class="rc-actions">
            <button type="button" class="rc-btn" onclick={() => (draft = null)}>
              {t('routines.aiDiscard')}
            </button>
            <button type="button" class="rc-btn rc-primary" disabled={busy} onclick={saveDraft}>
              {t('routines.aiSave')}
            </button>
          </div>
        {/if}
      {/if}
    </div>
  </div>
{/if}

<style>
  .rc-trigger {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 5px 9px;
    border-radius: 8px;
    border: 1px dashed var(--color-border-strong);
    background: transparent;
    color: var(--color-muted);
    font-size: 10.5px;
    font-weight: 600;
    cursor: pointer;
  }
  .rc-trigger:hover {
    color: var(--color-fg);
    border-color: var(--color-violet);
  }
  .rc-overlay {
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
  .rc-dialog {
    width: min(480px, calc(100vw - 40px));
    max-height: calc(100vh - 80px);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 18px 20px 16px;
    border-radius: 16px;
    border: 1px solid var(--color-border-strong);
    background: color-mix(in oklch, var(--color-surface) 96%, var(--color-bg));
    box-shadow: 0 30px 70px -24px var(--color-shadow);
  }
  .rc-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .rc-title {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
  }
  .rc-icon {
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
  .rc-icon:hover {
    color: var(--color-fg);
    background: var(--color-surface-2);
  }
  .rc-tabs {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 3px;
    padding: 3px;
    border-radius: 10px;
    background: color-mix(in oklch, var(--color-bg) 55%, transparent);
    box-shadow: inset 0 0 0 1px var(--color-border);
  }
  .rc-tab {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    height: 28px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--color-faint);
    font-size: 11.5px;
    font-weight: 600;
    cursor: pointer;
  }
  .rc-tab-on {
    background: color-mix(in oklch, var(--color-violet) 26%, var(--color-surface-2));
    color: var(--color-fg);
  }
  .rc-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 10.5px;
    font-weight: 600;
    color: var(--color-faint);
  }
  .rc-label {
    margin: 2px 0 0;
    font-size: 10.5px;
    font-weight: 600;
    color: var(--color-faint);
  }
  .rc-input {
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
  .rc-input:focus {
    border-color: var(--color-violet);
    box-shadow: 0 0 0 3px color-mix(in oklch, var(--color-violet) 16%, transparent);
  }
  .rc-textarea {
    height: auto;
    min-height: 64px;
    padding: 9px 10px;
    resize: vertical;
    line-height: 1.5;
    font-family: inherit;
  }
  .rc-field-inline {
    flex-direction: row;
    align-items: center;
    justify-content: space-between;
  }
  .rc-field-inline input {
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
  .rc-paths {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .rc-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 100%;
    padding: 3px 7px;
    border-radius: 8px;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
    font-size: 10.5px;
    color: var(--color-muted);
  }
  .rc-chip-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rc-chip button {
    display: inline-flex;
    align-items: center;
    border: none;
    background: transparent;
    color: var(--color-faint);
    cursor: pointer;
    padding: 0;
  }
  .rc-chip button:hover {
    color: var(--color-warn);
  }
  .rc-empty {
    margin: 0;
    font-size: 10.5px;
    color: var(--color-faint);
  }
  .rc-path-row {
    display: flex;
    gap: 6px;
  }
  .rc-path-row .rc-input {
    flex: 1;
  }
  .rc-btn {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 5px;
    padding: 6px 11px;
    white-space: nowrap;
    border-radius: 8px;
    border: 1px solid var(--color-border);
    background: transparent;
    color: var(--color-muted);
    font-size: 11px;
    font-weight: 600;
    cursor: pointer;
  }
  .rc-btn:hover {
    color: var(--color-fg);
  }
  .rc-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .rc-primary {
    border-color: color-mix(in oklch, var(--color-violet) 55%, transparent);
    color: var(--color-violet);
  }
  .rc-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 2px;
  }
  .rc-desc {
    margin: 0;
    font-size: 11px;
    line-height: 1.5;
    color: var(--color-faint);
  }
  .rc-draft {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 10px 12px;
    border-radius: 11px;
    border: 1px solid color-mix(in oklch, var(--color-violet) 35%, var(--color-border));
    background: color-mix(in oklch, var(--color-violet) 8%, var(--color-surface));
  }
  .rc-draft-title {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 0;
    font-size: 12.5px;
    font-weight: 620;
  }
  .rc-chip-soft {
    padding: 1px 6px;
    border-radius: 999px;
    border: 1px solid var(--color-border);
    font-size: 9.5px;
    color: var(--color-faint);
    font-weight: 600;
  }
  .rc-draft-path {
    margin: 0;
    font-size: 10.5px;
    color: var(--color-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rc-draft-reason {
    margin: 2px 0 0;
    font-size: 10.5px;
    color: var(--color-faint);
    line-height: 1.5;
  }
</style>

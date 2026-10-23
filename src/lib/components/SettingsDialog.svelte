<script lang="ts">
  import Icon from './Icon.svelte';
  import { store } from '../store.svelte';
  import { formatSize } from '../format';
  import { fade, fly } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { LOCALES, i18n, setLocale, t, type Locale } from '../i18n.svelte';

  function listIn(_el: Element, { index }: { index: number }) {
    return {
      duration: 300,
      delay: Math.min(index, 8) * 30,
      easing: cubicOut,
      css: (tm: number) =>
        `opacity:${tm};transform:translateY(${(12 * (1 - tm)).toFixed(1)}px)`
    };
  }

  // ---- AI profiles ----
  type AiProfileView = {
    id: string;
    name: string;
    provider: string;
    endpoint: string;
    model: string;
    hasToken: boolean;
  };

  let editorOpen = $state(false);
  let editor = $state({
    id: '',
    name: '',
    provider: '',
    endpoint: '',
    model: '',
    token: '',
    hasToken: false
  });
  let clearToken = $state(false);
  let saving = $state(false);

  // Local mirror of the scheduled hour. `bind:value` on a <select> needs a
  // state variable; binding the store property directly would not select the
  // matching <option> reliably (the attribute is applied before options
  // exist), which left the picker visibly empty.
  let scheduledHour = $state(store.scheduledHour);
  $effect(() => {
    scheduledHour = store.scheduledHour;
  });

  const presets = [
    {
      label: 'settings.aiPresetArk',
      provider: 'ark',
      endpoint: 'https://ark.cn-beijing.volces.com/api/v3/chat/completions',
    },
    {
      label: 'settings.aiPresetOpenai',
      provider: 'openai',
      endpoint: 'https://api.openai.com/v1/chat/completions',
      model: 'gpt-4o',
    },
    {
      // Official docs: base_url https://api.deepseek.com, route
      // /chat/completions, model deepseek-flash.
      label: 'settings.aiPresetDeepseek',
      provider: 'deepseek',
      endpoint: 'https://api.deepseek.com/chat/completions',
      model: 'deepseek-flash',
    },
    {
      // Anthropic uses /v1/messages with x-api-key, not the OpenAI route; the
      // model must be one the account can access.
      label: 'settings.aiPresetClaude',
      provider: 'anthropic',
      endpoint: 'https://api.anthropic.com/v1/messages',
    },
  ];

  function startProfile() {
    editor = {
      id: '',
      name: '',
      provider: 'deepseek',
      endpoint: 'https://api.deepseek.com/chat/completions',
      model: 'deepseek-flash',
      token: '',
      hasToken: false
    };
    clearToken = false;
    editorOpen = true;
  }

  function editProfile(profile: AiProfileView) {
    editor = {
      id: profile.id,
      name: profile.name,
      provider: profile.provider,
      endpoint: profile.endpoint,
      model: profile.model,
      token: '',
      hasToken: profile.hasToken
    };
    clearToken = false;
    editorOpen = true;
  }

  function applyPreset(preset: (typeof presets)[number]) {
    editor.provider = preset.provider;
    editor.endpoint = preset.endpoint;
    if (preset.model) editor.model = preset.model;
    if (!editor.name.trim()) editor.name = t(preset.label);
  }

  async function saveProfile() {
    if (saving) return;
    saving = true;
    const token = clearToken ? '' : editor.token !== '' ? editor.token : undefined;
    const ok = await store.saveAiProfile(
      {
        id: editor.id || null,
        name: editor.name,
        provider: editor.provider,
        endpoint: editor.endpoint,
        model: editor.model
      },
      token
    );
    saving = false;
    if (ok) editorOpen = false;
  }

  function providerLabel(provider: string) {
    const preset = presets.find((item) => item.provider === provider);
    return preset ? t(preset.label) : t('settings.aiProviderCustom');
  }

  function endpointHost(endpoint: string) {
    return endpoint.replace(/^https?:\/\//, '').split('/')[0];
  }

  async function save() {
    if (saving) return;
    saving = true;
    await store.setAiLanguage(i18n.current);
    saving = false;
    store.settingsOpen = false;
  }
</script>

{#if store.settingsOpen}
  <button
    type="button"
    aria-label={t('common.close')}
    class="set-backdrop"
    in:fade={{ duration: 180 }}
    out:fade={{ duration: 140 }}
    onclick={() => (store.settingsOpen = false)}
  ></button>

  <div
    class="set-sheet"
    role="dialog"
    aria-label={t('settings.title')}
    in:fly={{ y: 24, duration: 300, easing: cubicOut }}
    out:fly={{ y: 16, duration: 180 }}
  >
    <header class="set-head">
      <div>
        <h2 class="set-title">{t('settings.title')}</h2>
        <p class="set-sub">{t('settings.sub')}</p>
      </div>
      <button
        type="button"
        class="btn-icon"
        aria-label={t('common.close')}
        onclick={() => (store.settingsOpen = false)}
      >
        <Icon name="x" size={15} />
      </button>
    </header>

    <div class="set-scroll">
      <p class="set-section-label">{t('settings.general')}</p>

      <div class="set-card">
        <div class="set-card-info">
          <p class="set-card-title">{t('settings.language')}</p>
          <p class="set-card-desc">{t('settings.languageDesc')}</p>
        </div>
        <select
          class="lang-select"
          aria-label={t('settings.language')}
          onchange={(event) =>
            setLocale(event.currentTarget.value as Locale)}
        >
          {#each LOCALES as locale (locale.tag)}
            <option value={locale.tag} selected={i18n.current === locale.tag}>
              {locale.native}
            </option>
          {/each}
        </select>
      </div>

      <p class="set-section-label" style="margin-top: 22px">{t('settings.ai')}</p>

      <div class="set-card">
        <div class="set-card-info">
          <p class="set-card-title">{t('settings.aiEnableTitle')}</p>
          <p class="set-card-desc">{t('settings.aiEnableDesc')}</p>
        </div>
        <button
          type="button"
          class="switch"
          class:switch-on={store.aiConfig?.enabled}
          role="switch"
          aria-label={t('settings.aiEnableTitle')}
          aria-checked={store.aiConfig?.enabled}
          onclick={() => store.setAiEnabled(!store.aiConfig?.enabled)}
        >
          <span class="switch-knob"></span>
        </button>
      </div>

      <div class="ai-form" class:ai-form-off={!store.aiConfig?.enabled}>
        <p class="ai-label">{t('settings.aiProfiles')}</p>
        <div class="ai-profiles">
          {#each store.aiConfig?.profiles ?? [] as profile (profile.id)}
            <div
              class="ai-profile"
              class:ai-profile-on={store.aiConfig?.activeProfile === profile.id}
            >
              <button
                type="button"
                class="ai-profile-main"
                aria-pressed={store.aiConfig?.activeProfile === profile.id}
                onclick={() => store.selectAiProfile(profile.id)}
              >
                <span
                  class="ai-radio"
                  class:ai-radio-on={store.aiConfig?.activeProfile === profile.id}
                ></span>
                <span class="ai-profile-text">
                  <span class="ai-profile-name">
                    {profile.name}
                    <span class="ai-tag">{providerLabel(profile.provider)}</span>
                  </span>
                  <span class="ai-profile-meta">
                    {endpointHost(profile.endpoint)} · {profile.model}
                  </span>
                </span>
              </button>
              <span class="ai-key-chip" class:ai-key-ok={profile.hasToken}>
                {profile.hasToken ? t('settings.aiKeySaved') : t('settings.aiKeyMissing')}
              </span>
              <button
                type="button"
                class="ai-mini"
                aria-label={t('settings.aiEditProfile')}
                title={t('settings.aiEditProfile')}
                onclick={() => editProfile(profile)}
              >
                <Icon name="sliders" size={12} />
              </button>
              <button
                type="button"
                class="ai-mini ai-mini-danger"
                aria-label={t('settings.aiDeleteProfile')}
                title={t('settings.aiDeleteProfile')}
                onclick={() => store.deleteAiProfile(profile.id)}
              >
                <Icon name="trash" size={12} />
              </button>
            </div>
          {/each}
          <button type="button" class="ai-add" onclick={startProfile}>
            + {t('settings.aiAddProfile')}
          </button>
        </div>

        {#if editorOpen}
          <div class="ai-editor">
            <p class="ai-label">{t('settings.aiProvider')}</p>
            <div class="ai-presets">
              {#each presets as preset (preset.provider)}
                <button
                  type="button"
                  class="ai-preset"
                  class:preset-on={editor.provider === preset.provider}
                  aria-pressed={editor.provider === preset.provider}
                  onclick={() => applyPreset(preset)}
                >
                  {t(preset.label)}
                </button>
              {/each}
            </div>

            <p class="ai-label">{t('settings.aiName')}</p>
            <input
              type="text"
              class="ai-input"
              placeholder={t('settings.aiNamePh')}
              bind:value={editor.name}
              autocomplete="off"
            />

            <p class="ai-label">{t('settings.aiEndpoint')}</p>
            <input
              type="text"
              class="ai-input"
              placeholder={t('settings.aiEndpointPh')}
              bind:value={editor.endpoint}
              spellcheck="false"
              autocomplete="off"
            />

            <p class="ai-label">{t('settings.aiModel')}</p>
            <input
              type="text"
              class="ai-input"
              placeholder={t('settings.aiModelPh')}
              bind:value={editor.model}
              spellcheck="false"
              autocomplete="off"
            />

            <p class="ai-label">
              {t('settings.aiToken')}
              <span class="ai-token-state" class:ai-token-ok={editor.hasToken && !clearToken}>
                {editor.hasToken && !clearToken
                  ? t('settings.aiTokenSaved')
                  : t('settings.aiTokenNone')}
              </span>
            </p>
            <input
              type="password"
              class="ai-input"
              placeholder={t('settings.aiTokenPh')}
              bind:value={editor.token}
              spellcheck="false"
              autocomplete="off"
            />
            {#if editor.hasToken}
              <label class="ai-clear">
                <input type="checkbox" bind:checked={clearToken} />
                {t('settings.aiTokenClear')}
              </label>
            {/if}

            <div class="ai-editor-actions">
              <button type="button" class="btn btn-ghost btn-sm" onclick={() => (editorOpen = false)}>
                {t('settings.ruleCancel')}
              </button>
              <button type="button" class="btn btn-sm" disabled={saving} onclick={saveProfile}>
                {t('settings.aiProfileSave')}
              </button>
            </div>
          </div>
        {/if}

        <p class="ai-consent">{t('settings.aiConsent')}</p>
      </div>

      <p class="set-section-label" style="margin-top: 22px">{t('settings.automation')}</p>

      <div class="set-group">
        <div class="set-card">
          <div class="set-card-info">
            <p class="set-card-title">{t('settings.dailyTitle')}</p>
            <p class="set-card-desc">{t('settings.dailyDesc')}</p>
          </div>
          <button
            type="button"
            class="switch"
            class:switch-on={store.autoOn}
            role="switch"
            aria-label={t('settings.dailyTitle')}
            aria-checked={store.autoOn}
            onclick={() => store.toggleAuto()}
          >
            <span class="switch-knob"></span>
          </button>
        </div>

        <div class="set-card">
          <div class="set-card-info">
            <p class="set-card-title">{t('settings.scheduledTitle')}</p>
            <p class="set-card-desc">{t('settings.scheduledDesc')}</p>
          </div>
          <div class="sched-controls">
            <select
              class="sched-time"
              aria-label={t('settings.scheduledTime')}
              bind:value={scheduledHour}
              disabled={!store.scheduledOn}
              onchange={() =>
                store.saveScheduled(store.scheduledOn, Number(scheduledHour))}
            >
              {#each Array.from({ length: 24 }, (_, h) => h) as h}
                <option value={h}>{String(h).padStart(2, '0')}:00</option>
              {/each}
            </select>
            <button
              type="button"
              class="switch"
              class:switch-on={store.scheduledOn}
              role="switch"
              aria-label={t('settings.scheduledTitle')}
              aria-checked={store.scheduledOn}
              onclick={() => store.saveScheduled(!store.scheduledOn, scheduledHour)}
            >
              <span class="switch-knob"></span>
            </button>
          </div>
        </div>
      </div>

    </div>

    <footer class="set-foot">
      <button type="button" class="set-save" disabled={saving} onclick={save}>
        {saving ? '…' : t('settings.save')}
      </button>
    </footer>
  </div>
{/if}

<style>
  .set-backdrop {
    position: fixed;
    inset: 0;
    z-index: 40;
    border: none;
    background: oklch(0 0 0 / 0.42);
    backdrop-filter: blur(3px);
    -webkit-backdrop-filter: blur(3px);
    cursor: default;
  }
  .set-sheet {
    position: fixed;
    z-index: 41;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    width: min(560px, calc(100vw - 40px));
    max-height: min(720px, calc(100vh - 48px));
    display: flex;
    flex-direction: column;
    border-radius: 18px;
    border: 1px solid var(--color-border-strong);
    background: color-mix(in oklch, var(--color-surface) 96%, var(--color-bg));
    box-shadow:
      0 1px 0 color-mix(in oklch, var(--color-sheen) 6%, transparent) inset,
      0 40px 90px -30px var(--color-shadow);
    overflow: hidden;
  }
  .set-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    padding: 20px 22px 14px;
    flex: none;
  }
  .set-title {
    margin: 0;
    font-size: 18px;
    font-weight: 650;
    letter-spacing: -0.02em;
  }
  .set-sub {
    margin: 3px 0 0;
    font-size: 12.5px;
    color: var(--color-faint);
  }
  .set-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 22px 8px;
  }
  .set-section-label {
    margin: 0 0 14px;
    font-family: var(--font-mono);
    font-size: 10.5px;
    font-weight: 650;
    letter-spacing: 0.12em;
    color: var(--color-faint);
  }
  .set-card {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 16px 17px;
    border-radius: 14px;
    border: 1px solid var(--color-border);
    background: color-mix(in oklch, var(--color-surface-2) 55%, transparent);
  }
  /* Related rows share one outline and a single hairline between them.
     Two independently rounded 1px-bordered cards stacked with no gap double
     the border and leave radius notches at the seam. */
  .set-group {
    border: 1px solid var(--color-border);
    border-radius: 14px;
    overflow: hidden;
    background: color-mix(in oklch, var(--color-surface-2) 55%, transparent);
  }
  .set-group .set-card {
    border: none;
    border-radius: 0;
    background: transparent;
  }
  .set-group .set-card + .set-card {
    border-top: 1px solid var(--color-border);
  }
  .set-card-info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .set-card-title {
    margin: 0;
    font-size: 14px;
    font-weight: 620;
  }
  .set-card-desc {
    margin: 0;
    font-size: 11.5px;
    color: var(--color-faint);
  }
  .switch {
    position: relative;
    width: 44px;
    height: 26px;
    flex: none;
    border-radius: 999px;
    border: none;
    background: var(--color-surface-3);
    cursor: default;
    transition: background 0.2s ease, box-shadow 0.2s ease;
  }
  .switch.switch-on {
    background: var(--color-violet);
    box-shadow: 0 0 0 3px color-mix(in oklch, var(--color-violet) 20%, transparent);
  }
  .switch-knob {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--color-sheen);
    box-shadow: 0 2px 5px -1px color-mix(in oklch, var(--color-shadow) 60%, transparent);
    transition: transform 0.22s cubic-bezier(0.4, 0, 0.2, 1);
  }
  .switch.switch-on .switch-knob {
    transform: translateX(18px);
  }
  .switch:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--color-surface), 0 0 0 4px var(--color-violet);
  }
  .sched-controls {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: none;
  }
  .lang-select {
    height: 30px;
    max-width: 190px;
    padding: 0 9px;
    border-radius: 9px;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
    color: var(--color-fg);
    font-size: 12px;
    font-weight: 600;
    outline: none;
    cursor: pointer;
  }
  .lang-select:focus-visible {
    border-color: var(--color-violet);
    box-shadow: 0 0 0 3px color-mix(in oklch, var(--color-violet) 16%, transparent);
  }

  .sched-time {
    height: 28px;
    padding: 0 8px;
    border-radius: 9px;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
    color: var(--color-fg);
    font-family: var(--font-mono);
    font-size: 11.5px;
    font-weight: 600;
    outline: none;
  }
  .sched-time:not(:disabled) {
    cursor: pointer;
  }
  .sched-time:disabled {
    opacity: 0.5;
  }
  .sched-time:focus-visible {
    border-color: var(--color-violet);
    box-shadow: 0 0 0 3px color-mix(in oklch, var(--color-violet) 16%, transparent);
  }

  .ai-form {
    margin-top: 12px;
    padding: 12px 14px 2px;
    border-radius: 14px;
    border: 1px solid var(--color-border);
    background: color-mix(in oklch, var(--color-surface-2) 40%, transparent);
    transition: opacity 0.2s ease;
  }
  .ai-form-off {
    opacity: 0.45;
  }
  .ai-label {
    margin: 0 0 7px;
    font-size: 11px;
    font-weight: 620;
    color: var(--color-faint);
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .ai-presets {
    display: flex;
    gap: 8px;
    margin-bottom: 11px;
  }
  .ai-preset {
    padding: 6px 12px;
    border-radius: 9px;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
    color: var(--color-muted);
    font-size: 11.5px;
    font-weight: 600;
    cursor: default;
  }
  .ai-preset:hover {
    color: var(--color-fg);
    border-color: var(--color-border-strong);
  }
  .ai-preset.preset-on {
    color: var(--color-violet);
    border-color: color-mix(in oklch, var(--color-violet) 55%, transparent);
    background: color-mix(in oklch, var(--color-violet) 12%, var(--color-surface));
  }
  .ai-input {
    width: 100%;
    height: 32px;
    margin-bottom: 11px;
    padding: 0 12px;
    border-radius: 10px;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
    color: var(--color-fg);
    font-family: var(--font-mono);
    font-size: 11.5px;
    outline: none;
  }
  .ai-input:focus {
    border-color: var(--color-violet);
    box-shadow: 0 0 0 3px color-mix(in oklch, var(--color-violet) 16%, transparent);
  }
  .ai-token-state {
    font-size: 10px;
    font-weight: 600;
    color: var(--color-faint);
  }
  .ai-token-ok {
    color: var(--color-ok);
  }
  .ai-clear {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: -4px 0 12px;
    font-size: 11px;
    color: var(--color-faint);
    cursor: default;
  }
  .ai-consent {
    margin: 0 0 10px;
    font-size: 10.5px;
    line-height: 1.6;
    color: var(--color-faint);
  }
  .set-foot {
    flex: none;
    padding: 14px 24px 20px;
  }
  .set-save {
    width: 100%;
    height: 38px;
    border: none;
    border-radius: 11px;
    background: var(--color-cta-bg);
    color: var(--color-cta-fg);
    font-size: 13px;
    font-weight: 620;
    cursor: default;
  }
  .set-save:disabled {
    opacity: 0.6;
  }


  .ai-profiles {
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .ai-profile {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 7px 9px;
    border-radius: 10px;
    border: 1px solid var(--color-border);
    background: var(--color-surface);
  }
  .ai-profile-on {
    border-color: color-mix(in oklch, var(--color-violet) 55%, transparent);
    background: color-mix(in oklch, var(--color-violet) 10%, var(--color-surface));
  }
  .ai-profile-main {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 9px;
    border: none;
    background: transparent;
    padding: 0;
    text-align: left;
    cursor: pointer;
    color: inherit;
  }
  .ai-radio {
    flex: none;
    width: 13px;
    height: 13px;
    border-radius: 50%;
    border: 1.5px solid var(--color-border-strong);
  }
  .ai-radio-on {
    border-color: var(--color-violet);
    box-shadow: inset 0 0 0 3px var(--color-violet);
  }
  .ai-profile-text {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .ai-profile-name {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12.5px;
    font-weight: 620;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .ai-tag {
    padding: 1px 6px;
    border-radius: 999px;
    border: 1px solid var(--color-border);
    font-size: 9.5px;
    font-weight: 600;
    color: var(--color-faint);
  }
  .ai-profile-meta {
    font-size: 10.5px;
    color: var(--color-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .ai-key-chip {
    flex: none;
    padding: 2px 7px;
    border-radius: 999px;
    border: 1px solid var(--color-border);
    font-size: 9.5px;
    color: var(--color-faint);
  }
  .ai-key-ok {
    color: var(--color-violet);
    border-color: color-mix(in oklch, var(--color-violet) 45%, transparent);
  }
  .ai-mini {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border-radius: 7px;
    border: 1px solid transparent;
    background: transparent;
    color: var(--color-faint);
    cursor: pointer;
  }
  .ai-mini:hover {
    color: var(--color-fg);
    border-color: var(--color-border);
  }
  .ai-mini-danger:hover {
    color: var(--color-warn);
  }
  .ai-add {
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
  .ai-add:hover {
    color: var(--color-fg);
    border-color: var(--color-violet);
  }
  .ai-editor {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
    border-radius: 12px;
    border: 1px solid var(--color-border);
    background: color-mix(in oklch, var(--color-surface-2) 60%, transparent);
  }
  .ai-editor-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 2px;
  }
</style>

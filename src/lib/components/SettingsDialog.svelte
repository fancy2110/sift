<script lang="ts">
  import Icon from './Icon.svelte';
  import { store } from '../store.svelte';
  import { formatSize } from '../format';
  import { fade, fly } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';
  import { i18n, setLocale, t } from '../i18n.svelte';

  function listIn(_el: Element, { index }: { index: number }) {
    return {
      duration: 300,
      delay: Math.min(index, 8) * 30,
      easing: cubicOut,
      css: (tm: number) =>
        `opacity:${tm};transform:translateY(${(12 * (1 - tm)).toFixed(1)}px)`
    };
  }

  function toggleLanguage() {
    setLocale(i18n.current === 'zh' ? 'en' : 'zh');
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
        <button type="button" class="btn btn-ghost btn-sm" onclick={toggleLanguage}>
          {i18n.current === 'zh' ? 'English' : '中文'}
        </button>
      </div>

      <p class="set-section-label" style="margin-top: 22px">{t('settings.automation')}</p>

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

      <p class="set-section-label" style="margin-top: 22px">{t('settings.routines')}</p>

      <ul class="set-rt-list">
        {#each store.routines as r, i (r.id)}
          <li class="set-rt" in:listIn={{ index: i }}>
            <span class="set-rt-icon" class:set-rt-icon-on={r.mode === 'auto'}>
              <Icon name={r.mode === 'auto' ? 'bolt' : 'clock'} size={14} />
            </span>
            <button
              type="button"
              class="set-rt-info"
              title={t('settings.toggleMode')}
              onclick={() => store.toggleSavedRoutineMode(r.id)}
            >
              <span class="set-rt-name">{r.title}</span>
              <span class="set-rt-cad">
                {t(r.cadence)} · ≈{formatSize(r.averageBytes)} ·
                {r.mode === 'auto' ? t('routine.mode.auto') : t('routine.mode.approve')}
              </span>
            </button>
            <button
              type="button"
              class="btn-icon routine-run"
              title={t('settings.runNow')}
              aria-label={t('settings.runNow')}
              onclick={() => store.runSavedRoutine(r.id)}
            >
              <Icon name="bolt" size={13} />
            </button>
            <button
              type="button"
              class="btn-icon routine-del"
              title={t('settings.delete')}
              aria-label={t('settings.delete')}
              onclick={() => store.deleteSavedRoutine(r.id)}
            >
              <Icon name="trash" size={13} />
            </button>
          </li>
        {:else}
          <li class="set-empty">{t('routines.empty2')}</li>
        {/each}
      </ul>
    </div>

    <footer class="set-foot">
      <button type="button" class="set-save" onclick={() => (store.settingsOpen = false)}>
        {t('settings.save')}
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
    max-height: min(640px, calc(100vh - 60px));
    display: flex;
    flex-direction: column;
    border-radius: 18px;
    border: 1px solid var(--color-border-strong);
    background: color-mix(in oklch, var(--color-surface) 96%, var(--color-bg));
    box-shadow:
      0 1px 0 color-mix(in oklch, white 6%, transparent) inset,
      0 40px 90px -30px black;
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
    padding: 0 22px;
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
    background: #fff;
    box-shadow: 0 2px 5px -1px oklch(0% 0 0 / 0.5);
    transition: transform 0.22s cubic-bezier(0.4, 0, 0.2, 1);
  }
  .switch.switch-on .switch-knob {
    transform: translateX(18px);
  }
  .switch:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--color-surface), 0 0 0 4px var(--color-violet);
  }
  .set-rt-list {
    list-style: none;
    margin: 0;
    padding: 0 0 4px;
  }
  .set-rt {
    display: flex;
    align-items: center;
    gap: 11px;
    padding: 9px 4px;
    border-bottom: 1px solid color-mix(in oklch, var(--color-border) 60%, transparent);
  }
  .set-rt:last-child {
    border-bottom: none;
  }
  .set-rt-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 30px;
    height: 30px;
    border-radius: 9px;
    color: var(--color-muted);
    background: var(--color-surface-2);
    flex: none;
  }
  .set-rt-icon-on {
    color: var(--color-accent-hi);
    background: color-mix(in oklch, var(--color-accent) 15%, transparent);
  }
  .set-rt-info {
    flex: 1;
    min-width: 0;
    border: none;
    background: transparent;
    text-align: left;
    cursor: default;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .set-rt-name {
    font-size: 12.5px;
    font-weight: 620;
    color: var(--color-fg);
  }
  .set-rt-cad {
    font-size: 10.5px;
    color: var(--color-faint);
  }
  .set-empty {
    padding: 12px 4px;
    font-size: 12px;
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
    background: #f4f4f6;
    color: #141418;
    font-size: 13px;
    font-weight: 620;
    cursor: default;
  }
</style>

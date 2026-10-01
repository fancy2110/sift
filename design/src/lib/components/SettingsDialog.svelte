<script lang="ts">
  import Icon from './Icon.svelte';
  import { store } from '../store.svelte';
  import { formatSize } from '../format';
  import { fly, fade } from 'svelte/transition';
  import { cubicOut } from 'svelte/easing';

  function listIn(_el: Element, { index }: { index: number }) {
    return {
      duration: 300,
      delay: Math.min(index, 8) * 30,
      easing: cubicOut,
      css: (t: number) =>
        `opacity:${t};transform:translateY(${(12 * (1 - t)).toFixed(1)}px) scale(${(0.99 + 0.01 * t).toFixed(4)})`
    };
  }
</script>

{#if store.settingsOpen}
  <button
    type="button"
    aria-label="关闭"
    class="set-backdrop"
    in:fade={{ duration: 180 }}
    out:fade={{ duration: 140 }}
    onclick={() => (store.settingsOpen = false)}
  ></button>

  <div
    class="set-sheet"
    role="dialog"
    aria-label="系统设置"
    in:fly={{ y: 24, duration: 300, easing: cubicOut }}
    out:fly={{ y: 16, duration: 180 }}
    data-od-id="settings-sheet"
  >
    <header class="set-head">
      <div>
        <h2 class="set-title">系统设置</h2>
        <p class="set-sub">配置 AI 整理引擎与自动化</p>
      </div>
      <button
        type="button"
        class="btn-icon"
        aria-label="关闭"
        onclick={() => (store.settingsOpen = false)}
      >
        <Icon name="x" size={15} />
      </button>
    </header>

    <div class="set-scroll">
      <p class="set-section-label">自动化</p>

      <div class="set-card">
        <div class="set-card-info">
          <p class="set-card-title">每日自动整理</p>
          <p class="set-card-desc">每天凌晨 04:00 运行引擎，自动清理安全项</p>
        </div>
        <button
          type="button"
          class="switch"
          class:switch-on={store.autoOn}
          role="switch"
          aria-label="每日自动整理"
          aria-checked={store.autoOn}
          onclick={() => store.toggleAuto()}
          data-od-id="master-switch"
        >
          <span class="switch-knob"></span>
        </button>
      </div>

      <p class="set-section-label" style="margin-top: 22px">例行任务</p>

      <ul class="set-rt-list">
        {#each store.routines as r, i (r.id)}
          <li class="set-rt" in:listIn={{ index: i }} out:fly={{ x: -24, duration: 180 }}>
            <span class="set-rt-icon" class:set-rt-icon-on={r.autoMode === 'auto'}>
              {#if store.runningRoutineId === r.id}
                <Icon name="refresh" size={14} class="spin" />
              {:else}
                <Icon name={r.autoMode === 'auto' ? 'bolt' : 'clock'} size={14} />
              {/if}
            </span>
            <button
              type="button"
              class="set-rt-info"
              title="切换自动 / 确认"
              onclick={() => store.toggleRoutineMode(r.id)}
            >
              <span class="set-rt-name">{r.title}</span>
              <span class="set-rt-cad"
                >{r.cadence} · ≈{formatSize(r.avgSize)} ·
                {r.autoMode === 'auto' ? '自动执行' : '执行前确认'}</span
              >
            </button>
            <button
              type="button"
              class="btn-icon routine-run"
              title="立即启动"
              aria-label="启动{r.title}"
              disabled={store.runningRoutineId !== null}
              onclick={() => store.runRoutine(r.id)}
            >
              <Icon name="bolt" size={13} />
            </button>
            <button
              type="button"
              class="btn-icon routine-del"
              title="删除"
              aria-label="删除{r.title}"
              onclick={() => store.deleteRoutine(r.id)}
            >
              <Icon name="trash" size={13} />
            </button>
          </li>
        {:else}
          <li class="set-empty">重复的整理决策会在这里自动沉淀</li>
        {/each}
      </ul>
    </div>

    <footer class="set-foot">
      <button type="button" class="set-save" onclick={() => (store.settingsOpen = false)}>
        保存更改
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
    padding: 4px 22px 18px;
  }
  .set-section-label {
    margin: 0 0 10px;
    font-family: var(--font-mono);
    font-size: 10.5px;
    font-weight: 650;
    letter-spacing: 0.16em;
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

  /* switch */
  .switch {
    position: relative;
    width: 44px;
    height: 26px;
    flex: none;
    border: none;
    border-radius: 999px;
    background: var(--color-surface-3);
    cursor: default;
    transition: background 0.2s ease, box-shadow 0.2s ease;
  }
  .switch-on {
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
  .switch-on .switch-knob {
    transform: translateX(18px);
  }
  .switch:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--color-surface), 0 0 0 4px var(--color-violet);
  }

  .btn-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 30px;
    height: 30px;
    flex: none;
    border: none;
    border-radius: 9px;
    background: transparent;
    color: var(--color-muted);
    cursor: default;
    transition: background 0.15s ease, color 0.15s ease;
  }
  .btn-icon:hover {
    background: color-mix(in oklch, var(--color-surface-3) 70%, transparent);
    color: var(--color-fg);
  }
  .routine-run:hover {
    color: var(--color-accent-hi);
    background: color-mix(in oklch, var(--color-accent) 14%, transparent);
  }
  .routine-del:hover {
    color: var(--color-danger, #ff6b6b);
    background: color-mix(in oklch, #ff453a 14%, transparent);
  }
  .btn-icon:disabled {
    opacity: 0.4;
  }
  .btn-icon:focus-visible {
    outline: 2px solid color-mix(in oklch, var(--color-accent) 70%, transparent);
    outline-offset: 2px;
  }

  .set-rt-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .set-rt {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border-radius: 11px;
    border: 1px solid var(--color-border);
    background: color-mix(in oklch, var(--color-surface-2) 45%, transparent);
  }
  .set-rt-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    flex: none;
    border-radius: 9px;
    color: var(--color-muted);
    background: color-mix(in oklch, var(--color-surface-3) 70%, transparent);
  }
  .set-rt-icon-on {
    color: var(--color-accent-hi);
    background: color-mix(in oklch, var(--color-accent) 17%, transparent);
  }
  .set-rt-info {
    flex: 1;
    min-width: 0;
    border: none;
    background: transparent;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    text-align: left;
    cursor: default;
  }
  .set-rt-name {
    font-size: 12.5px;
    font-weight: 620;
    color: var(--color-fg);
  }
  .set-rt-cad {
    font-size: 10.5px;
    color: var(--color-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .set-empty {
    padding: 16px;
    font-size: 12px;
    color: var(--color-faint);
    text-align: center;
  }
  .set-foot {
    flex: none;
    display: flex;
    justify-content: flex-end;
    padding: 12px 22px 18px;
    border-top: 1px solid color-mix(in oklch, var(--color-border) 70%, transparent);
  }
  .set-save {
    display: inline-flex;
    align-items: center;
    height: 38px;
    padding: 0 22px;
    border: none;
    border-radius: 11px;
    background: var(--color-violet);
    color: #fff;
    font-size: 13.5px;
    font-weight: 600;
    cursor: default;
    transition: background 0.15s ease, transform 0.15s ease;
  }
  .set-save:hover {
    background: color-mix(in oklch, var(--color-violet) 86%, white);
    transform: translateY(-1px);
  }
  .set-save:focus-visible {
    outline: 2px solid color-mix(in oklch, var(--color-violet) 60%, white);
    outline-offset: 3px;
  }
</style>

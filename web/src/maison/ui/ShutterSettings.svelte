<script>
  import { onMount } from 'svelte';
  import { device, value, act } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** The durations of one shutter: how long it takes each way, and its
   *  slats. Each is sent as soon as it is typed; the driver keeps it. */
  let { id, label, onclose } = $props();

  const KEYS = ['travel_up', 'travel_down', 'slack_up', 'slack_down'];
  const points = $derived(KEYS.map((key) => device(id)?.points.find((p) => p.key === key)).filter(Boolean));

  let dialog;
  onMount(() => dialog.showModal());

  function send(p, e) {
    const secs = Number(e.currentTarget.value);
    const { min, max } = p.kind;
    if (e.currentTarget.value === '' || !Number.isFinite(secs) || secs < min || secs > max) {
      e.currentTarget.value = value(id, p.key) ?? '';
      return;
    }
    act(`${id}/${p.key}`, secs, t('commun.action', { label, action: p.label.toLowerCase() }));
  }
</script>

<dialog bind:this={dialog} {onclose} onclick={(e) => e.target === dialog && dialog.close()} aria-label={t('commun.volet.reglages')}>
  <header>
    <div>
      <p class="eyebrow">{label}</p>
      <h2>{t('commun.volet.reglages')}</h2>
    </div>
    <button class="close" onclick={() => dialog.close()} aria-label={t('commun.fermer')}><Icon name="close" size={22} /></button>
  </header>
  <p class="help">{t('commun.volet.reglages_aide')}</p>
  {#each points as p (p.key)}
    <label>
      <span>{p.label}</span>
      <span class="field">
        <input type="number" inputmode="decimal" min={p.kind.min} max={p.kind.max} step={p.kind.step} value={value(id, p.key)} onchange={(e) => send(p, e)} />
        <small>s</small>
      </span>
    </label>
  {/each}
</dialog>

<style>
  dialog {
    border: 0;
    padding: 22px;
    width: min(420px, 96vw);
    max-height: 94dvh;
    overflow-y: auto;
    border-radius: var(--r-xl);
    background: var(--surface);
    color: var(--ink);
    box-shadow: 0 30px 80px rgb(0 0 0 / 40%);
  }

  dialog::backdrop {
    background: rgb(4 6 12 / 60%);
    backdrop-filter: blur(4px);
  }

  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }

  h2 {
    margin: 2px 0 0;
  }

  .eyebrow,
  .help {
    margin: 0;
    color: var(--ink-2);
    font-size: 0.85rem;
  }

  .help {
    margin: 12px 0 16px;
  }

  .close {
    border: 0;
    background: transparent;
    color: var(--ink-2);
    cursor: pointer;
    padding: 4px;
  }

  label {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 0;
  }

  .field {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  input {
    width: 84px;
    padding: 8px 10px;
    border: 1px solid var(--surface-2);
    border-radius: 10px;
    background: var(--surface-2);
    color: var(--ink);
    font: inherit;
    text-align: right;
  }
</style>

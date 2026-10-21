<script>
  import { device, nameOf, reachable, value, act, pending, num, deviceKind } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** Any device the dashboard has no card for: what it can be told (a switch,
   *  choices, a number, a message to send) and what it reports. */
  let { id, name = null } = $props();

  const d = $derived(device(id));
  const ICON = { plug: 'power', tv: 'remote', speaker: 'speaker', camera: 'cctv', sensor: 'info', printer: 'printer3d', cover: 'home-roof' };
  // The pairing's own steps and the like stay on their settings page.
  const HIDDEN = /^(linkquality|update|identify|countdown|cycle_time|random_time|switch_inching|relay_status|light_mode|child_lock|sas|forget|pairing|link|invitation|code|contacts)/;

  const points = $derived((d?.points ?? []).filter((p) => !HIDDEN.test(p.key)));
  const controls = $derived(points.filter((p) => p.access?.write));
  const readings = $derived(points.filter((p) => !p.access?.write).slice(0, 5));

  const typeOf = (p) => p.kind?.type;
  const label = (p) => p.label || p.key.replace(/_/g, ' ');
  const say = (p) => {
    const v = value(id, p.key);
    if (v == null || v === '') return '—';
    if (typeof v === 'boolean') return v ? t('commun.oui') : t('commun.non');
    if (typeof v === 'number') return `${num(v, Math.abs(v) < 10 && !Number.isInteger(v) ? 1 : 0)}${p.unit ? ` ${p.unit}` : ''}`;
    return String(v);
  };

  /** what is being typed, point by point */
  let typed = $state({});
  async function send(p) {
    const text = (typed[p.key] ?? '').trim();
    if (!text) return;
    if ((await act(`${id}/${p.key}`, text, label(p))) !== 'error') typed[p.key] = '';
  }
</script>

<section class="card device" class:off={!reachable(id)}>
  <div class="card-head">
    <h2><Icon name={ICON[deviceKind(d)] ?? 'info'} size={18} />{nameOf(id, name)}</h2>
  </div>
  {#each controls as p (p.key)}
    <div class="row">
      {#if typeOf(p) === 'binary'}
        <span>{label(p)}</span>
        <button class="switch" class:on={value(id, p.key) === true} role="switch" aria-checked={value(id, p.key) === true} aria-label={label(p)} disabled={pending(id, p.key)} onclick={() => act(`${id}/${p.key}`, value(id, p.key) !== true, label(p))}>
          <i></i>
        </button>
      {:else if typeOf(p) === 'enum'}
        <span>{label(p)}</span>
        <div class="chips">
          {#each p.kind.values as v (v)}
            <button class:on={value(id, p.key) === v} onclick={() => act(`${id}/${p.key}`, v, label(p))}>{v}</button>
          {/each}
        </div>
      {:else if typeOf(p) === 'numeric'}
        <span>{label(p)} <b class="num">{say(p)}</b></span>
        <input type="range" min={p.kind.min ?? 0} max={p.kind.max ?? 100} step={p.kind.step ?? 1} value={value(id, p.key) ?? p.kind.min ?? 0} aria-label={label(p)} onchange={(e) => act(`${id}/${p.key}`, Number(e.currentTarget.value), label(p))} />
      {:else}
        <form onsubmit={(e) => (e.preventDefault(), send(p))}>
          <input bind:value={typed[p.key]} placeholder={label(p)} aria-label={label(p)} />
          <button type="submit" disabled={!(typed[p.key] ?? '').trim()} aria-label={t('maison.dashboard.envoyer')}><Icon name="arrow-top-right" size={16} /></button>
        </form>
      {/if}
    </div>
  {/each}
  {#if readings.length}
    <dl>
      {#each readings as p (p.key)}
        <div><dt>{label(p)}</dt><dd class="num">{say(p)}</dd></div>
      {/each}
    </dl>
  {/if}
  {#if !controls.length && !readings.length}
    <p class="muted">{t('commun.capteur.pas_de_mesure')}</p>
  {/if}
</section>

<style>
  .device {
    display: grid;
    gap: 12px;
    align-content: start;
  }

  .off {
    opacity: 0.6;
  }

  .row {
    display: grid;
    gap: 8px;
    font-size: 14px;
  }

  .row:has(.switch) {
    grid-template-columns: 1fr auto;
    align-items: center;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .chips button,
  form button {
    border: 0;
    border-radius: 999px;
    padding: 7px 12px;
    font: inherit;
    font-size: 13px;
    font-weight: 650;
    background: var(--surface-2);
    color: var(--ink-3);
    cursor: pointer;
  }

  .chips button.on {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .switch {
    width: 46px;
    height: 28px;
    padding: 3px;
    border: 0;
    border-radius: 999px;
    background: var(--surface-2);
    cursor: pointer;
  }

  .switch i {
    display: block;
    width: 22px;
    height: 22px;
    border-radius: 50%;
    background: var(--ink-3);
    transition: transform 0.15s;
  }

  .switch.on {
    background: var(--warm-soft);
  }

  .switch.on i {
    background: var(--warm-ink);
    transform: translateX(18px);
  }

  form {
    display: flex;
    gap: 8px;
  }

  form input {
    flex: 1;
    min-width: 0;
    border: 0;
    border-radius: 999px;
    padding: 9px 14px;
    font: inherit;
    font-size: 14px;
    background: var(--surface-2);
    color: var(--ink);
  }

  dl {
    display: grid;
    gap: 6px;
    margin: 0;
    font-size: 14px;
  }

  dl div {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }

  dt {
    color: var(--ink-3);
  }

  dd {
    margin: 0;
  }
</style>

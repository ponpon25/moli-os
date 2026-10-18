<script>
  import { value, act, pending } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** The house's alarm: what it says, and three big buttons — disarm,
   *  night (partial), full. An order is held by the guard until the
   *  household's code is given (the dashboard asks). */
  let { id } = $props();

  const BUTTONS = [
    { mode: 'disarmed', icon: 'shield-off', tone: 'ok' },
    { mode: 'partial', icon: 'shield-moon', tone: 'warn' },
    { mode: 'total', icon: 'shield-lock', tone: 'alert' },
  ];
  const state = $derived(value(id, 'state'));
  const mode = $derived(value(id, 'mode'));
  const last = $derived(value(id, 'last_order'));
  const busy = $derived(pending(id, 'mode'));

  // How the state looks: calm green, partial orange, full red.
  const tone = $derived(
    !state || state === 'unknown'
      ? 'idle'
      : state === 'disarmed'
        ? 'ok'
        : state.startsWith('total')
          ? 'alert'
          : 'warn',
  );
  const shown = $derived(tone === 'ok' ? 'shield-off' : tone === 'alert' ? 'shield-lock' : tone === 'warn' ? 'shield-moon' : 'shield');

  /** A word of the catalogue, or the raw value when it has none. */
  function word(prefix, key) {
    const k = `maison.alarme.${prefix}.${key}`;
    const w = t(k);
    return w === k ? String(key) : w;
  }

  function choose(m) {
    if (m === mode || busy) return;
    act(`${id}/mode`, m, t('maison.alarme.action', { mode: word('mode', m) }));
  }
</script>

<section class="card alarm {tone}">
  <div class="status">
    <span class="badge"><Icon name={shown} size={34} /></span>
    <div class="words">
      <h2>{t('maison.alarme.titre')}</h2>
      <b class="state">{state ? word('etat', state) : t('maison.alarme.inconnu')}</b>
      {#if last}<span class="last">{last}</span>{/if}
    </div>
  </div>
  <div class="buttons" role="group" aria-label={t('maison.alarme.modes')}>
    {#each BUTTONS as b (b.mode)}
      <button class="big {b.tone}" class:on={b.mode === mode} aria-pressed={b.mode === mode} disabled={busy} onclick={() => choose(b.mode)}>
        <Icon name={b.icon} size={34} />
        <span>{word('mode', b.mode)}</span>
      </button>
    {/each}
  </div>
</section>

<style>
  .alarm {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: 14px 28px;
    --tone: var(--ink-3);
  }

  .alarm.ok {
    --tone: #2f9e55;
  }

  .alarm.warn {
    --tone: var(--warm, #e08a00);
  }

  .alarm.alert {
    --tone: var(--alert, #e5482b);
  }

  .status {
    display: flex;
    align-items: center;
    gap: 18px;
    min-width: 0;
  }

  .badge {
    flex: none;
    width: 72px;
    height: 72px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--tone) 16%, var(--surface-2));
    color: var(--tone);
    box-shadow: 0 0 0 6px color-mix(in srgb, var(--tone) 10%, transparent);
    transition: background 0.3s var(--ease), color 0.3s var(--ease);
  }

  .words {
    display: grid;
    gap: 2px;
    min-width: 0;
  }

  h2 {
    font-size: 13px;
    font-weight: 650;
    color: var(--ink-3);
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .state {
    font-size: 26px;
    font-weight: 750;
    color: var(--tone);
  }

  .last {
    font-size: 12.5px;
    color: var(--ink-3);
  }

  .buttons {
    display: flex;
    gap: 12px;
  }

  .big {
    width: 112px;
    padding: 14px 8px 12px;
    border: 0;
    border-radius: 22px;
    display: grid;
    justify-items: center;
    gap: 8px;
    font: inherit;
    font-size: 13.5px;
    font-weight: 700;
    line-height: 1.15;
    text-align: center;
    background: var(--surface-2);
    color: var(--ink-2);
    cursor: pointer;
    transition: background 0.2s var(--ease), color 0.2s var(--ease), transform 0.15s var(--ease);
  }

  .big.ok {
    --b: #2f9e55;
  }

  .big.warn {
    --b: var(--warm, #e08a00);
  }

  .big.alert {
    --b: var(--alert, #e5482b);
  }

  .big :global(svg) {
    color: var(--b);
  }

  .big:hover:not(:disabled):not(.on) {
    background: color-mix(in srgb, var(--b) 14%, var(--surface-2));
    transform: translateY(-2px);
  }

  .big.on {
    background: var(--b);
    color: #fff;
  }

  .big.on :global(svg) {
    color: #fff;
  }

  .big:disabled {
    opacity: 0.6;
    cursor: progress;
  }

  @media (max-width: 900px) {
    .alarm {
      grid-template-columns: 1fr;
    }

    .buttons {
      justify-content: space-between;
    }

    .big {
      flex: 1;
      width: auto;
    }
  }
</style>

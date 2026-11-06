<script>
  import { device, nameOf, reachable, value, since, relative, home, act, pending, roomOf, num } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';
  import { newAlert } from '../lib/auto.svelte.js';

  /** A smoke detector, whatever its brand: calm when all is well, loud when not. */
  let { id, name = null } = $props();

  const d = $derived(device(id));
  const can = (key) => d?.points.some((p) => p.key === key && p.access?.write);

  const smoke = $derived(value(id, 'smoke') === true);
  // Its own test, or a siren someone started by hand: it rings, but no fire.
  const testing = $derived(value(id, 'test') === true || value(id, 'buzzer_manual_alarm') === true);
  const state = $derived(!reachable(id) ? 'off' : smoke ? 'alarm' : testing ? 'test' : value(id, 'smoke') == null ? 'unknown' : 'ok');

  const battery = $derived(value(id, 'battery') ?? value(id, 'battery_percentage'));
  const low = $derived(value(id, 'battery_low') === true || (typeof battery === 'number' && battery <= 15));
  const density = $derived(value(id, 'smoke_density'));
  // Zigbee link quality (0-255) in four bars.
  const lqi = $derived(value(id, 'linkquality'));
  const bars = $derived(typeof lqi === 'number' ? Math.min(4, Math.ceil(lqi / 64)) : null);
  const heard = $derived(Math.max(0, ...(d?.points ?? []).map((p) => since(id, p.key) ?? 0)));

  async function selftest() {
    if (!confirm(t('commun.fumee.tester_confirme'))) return;
    await act(`${id}/selftest`, 'selftest', t('commun.fumee.tester'));
  }
  const mute = () => act(`${id}/buzzer`, 'mute', t('commun.fumee.couper'));
</script>

<section class="card smoke {state}">
  <div class="card-head">
    <h2><Icon name="smoke-detector" size={18} />{nameOf(id, name)}{#if roomOf(d)}<small>{roomOf(d)}</small>{/if}</h2>
    <span class="side">
      {#if heard}<span class="when">{relative(heard, home.now)}</span>{/if}
      <button class="bell" onclick={() => newAlert(id)} title={t('commun.fumee.alertes')} aria-label={t('commun.fumee.alertes')}><Icon name="bell" size={16} /></button>
    </span>
  </div>

  <div class="hero" role="status" aria-live="assertive">
    <span class="halo" aria-hidden="true">
      <Icon name={state === 'alarm' ? 'fire' : state === 'off' ? 'offline' : state === 'test' ? 'bell' : 'shield'} size={30} />
    </span>
    <div class="words">
      <b>{t(`commun.fumee.${state}`)}</b>
      <span>
        {#if state === 'alarm' && density}{t('commun.fumee.densite', { value: num(density, 1) })}
        {:else}{t(`commun.fumee.${state}_aide`)}{/if}
      </span>
    </div>
  </div>

  <div class="foot">
    <ul class="facts">
      {#if typeof battery === 'number'}
        <li class:warn={low} title={t('commun.fumee.pile')}>
          <Icon name={low ? 'battery-low' : battery > 60 ? 'battery' : 'battery-medium'} size={16} />{num(battery)} %
        </li>
      {/if}
      {#if bars != null}
        <li title={t('commun.fumee.signal', { value: lqi })}>
          <span class="bars" aria-hidden="true">{#each [1, 2, 3, 4] as b (b)}<i class:on={b <= bars}></i>{/each}</span>
          {t(`commun.fumee.signal_${bars}`)}
        </li>
      {/if}
    </ul>
    {#if (state === 'alarm' || state === 'test') && can('buzzer')}
      <button class="act loud" onclick={mute} disabled={pending(id, 'buzzer')}><Icon name="volume-off" size={16} />{t('commun.fumee.couper')}</button>
    {:else if state !== 'off' && can('selftest')}
      <button class="act" onclick={selftest} disabled={pending(id, 'selftest')}><Icon name="bell" size={16} />{t('commun.fumee.tester')}</button>
    {/if}
  </div>
</section>

<style>
  .smoke {
    --tone: var(--good);
    --tone-soft: var(--good-soft);
    display: grid;
    gap: 18px;
    overflow: hidden;
  }

  .smoke.alarm {
    --tone: var(--alert);
    --tone-soft: var(--alert-soft);
    background: linear-gradient(160deg, var(--alert-soft), var(--surface) 70%);
  }

  .smoke.test {
    --tone: var(--warm);
    --tone-soft: var(--warm-soft);
  }

  .smoke.off,
  .smoke.unknown {
    --tone: var(--ink-3);
    --tone-soft: var(--surface-2);
  }

  .card-head {
    margin-bottom: 0;
  }

  h2 small {
    font-weight: 600;
    color: var(--ink-3);
  }

  h2 small::before {
    content: '·';
    margin-right: 8px;
  }

  .side {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .when {
    font-size: 13px;
    color: var(--ink-3);
  }

  .bell {
    width: 30px;
    height: 30px;
    display: grid;
    place-items: center;
    border: 0;
    border-radius: 50%;
    background: var(--surface-2);
    color: var(--ink-2);
    cursor: pointer;
  }

  .bell:hover {
    color: var(--warm-ink);
  }

  .hero {
    display: flex;
    align-items: center;
    gap: 16px;
  }

  .halo {
    position: relative;
    flex: none;
    width: 64px;
    height: 64px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    color: var(--tone);
    background: var(--tone-soft);
  }

  /* A slow breath when all is well, a pulse when it is not. */
  .halo::after {
    content: '';
    position: absolute;
    inset: -6px;
    border-radius: 50%;
    border: 2px solid var(--tone);
    opacity: 0.18;
    animation: breathe 4s var(--ease) infinite;
  }

  .alarm .halo,
  .test .halo {
    color: var(--on-ink);
    background: var(--tone);
  }

  .alarm .halo::after,
  .test .halo::after {
    opacity: 0.5;
    animation: pulse 1.1s ease-out infinite;
  }

  .off .halo::after,
  .unknown .halo::after {
    display: none;
  }

  @keyframes breathe {
    50% {
      transform: scale(1.06);
      opacity: 0.32;
    }
  }

  @keyframes pulse {
    to {
      transform: scale(1.35);
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .halo::after {
      animation: none;
    }
  }

  .words {
    display: grid;
    gap: 2px;
    min-width: 0;
  }

  .words b {
    font-size: 22px;
    font-weight: 750;
    letter-spacing: -0.01em;
  }

  .alarm .words b {
    color: var(--alert);
  }

  .words span {
    font-size: 14px;
    color: var(--ink-2);
  }

  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 10px;
  }

  .facts {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .facts li {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 11px;
    border-radius: 999px;
    background: var(--surface-2);
    font-size: 13px;
    font-weight: 650;
    color: var(--ink-2);
  }

  .facts li.warn {
    background: var(--alert-soft);
    color: var(--alert);
  }

  .bars {
    display: inline-flex;
    align-items: flex-end;
    gap: 2px;
    height: 12px;
  }

  .bars i {
    width: 3px;
    border-radius: 1px;
    background: var(--line);
  }

  .bars i:nth-child(1) { height: 25%; }
  .bars i:nth-child(2) { height: 50%; }
  .bars i:nth-child(3) { height: 75%; }
  .bars i:nth-child(4) { height: 100%; }

  .bars i.on {
    background: var(--ink-2);
  }

  .act {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 0;
    border-radius: 999px;
    padding: 8px 14px;
    font: inherit;
    font-size: 13px;
    font-weight: 700;
    background: var(--surface-2);
    color: var(--ink-2);
    cursor: pointer;
  }

  .act:hover {
    background: var(--surface-3);
  }

  .act.loud {
    background: var(--alert);
    color: #fff;
  }

  .act:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>

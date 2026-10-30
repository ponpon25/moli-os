<script>
  import { onMount } from 'svelte';
  import { device, value, since, reachable, nameOf, act, pending, relative, clock, home } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** A garage door: a small drawing, its slats rising and falling at the
   *  door's own pace. There is no sensor, so no ceremony: the whole card is
   *  one button, a tap opens or closes, and stops the door while it moves. */
  let { id, name = null } = $props();

  const d = $derived(device(id));
  const state = $derived(value(id, 'state') ?? 'closed');
  const target = $derived(value(id, 'target') ?? 'closed');
  const travel = $derived(Number(value(id, 'travel')) || 20);
  const assumed = $derived(value(id, 'assumed') === true);
  const online = $derived(reachable(id));
  const busy = $derived(pending(id, 'target') || pending(id, 'press'));
  const moving = $derived(state === 'opening' || state === 'closing');
  const changed = $derived(since(id, 'state'));

  // Where the door is drawn, 0 closed to 1 open: it follows the journey
  // from when it started, at the door's pace.
  let pos = $state(0);
  let from = 0;
  let frame;
  function follow() {
    cancelAnimationFrame(frame);
    const end = state === 'open' ? 1 : state === 'closed' ? 0 : null;
    if (end !== null) {
      pos = end;
      return;
    }
    if (state === 'stopped') return;
    const up = state === 'opening';
    from = pos;
    const started = changed ?? Date.now();
    const tick = () => {
      const done = Math.max(0, (Date.now() - started) / 1000 / travel);
      pos = up ? Math.min(1, from + done) : Math.max(0, from - done);
      if ((up && pos < 1) || (!up && pos > 0)) frame = requestAnimationFrame(tick);
    };
    tick();
  }
  $effect(() => {
    void state;
    void changed;
    follow();
  });
  onMount(() => {
    // A door found stopped half-way: drawn half-way.
    if (state === 'stopped') pos = 0.5;
    return () => cancelAnimationFrame(frame);
  });

  const word = $derived(t(`commun.garage.etat.${state}`));
  const next = $derived(state === 'open' || (state === 'stopped' && target === 'open') ? 'closed' : 'open');

  const label = $derived(nameOf(id, name));
  const verb = $derived(moving ? 'commun.garage.arreter' : next === 'open' ? 'commun.garage.ouvrir' : 'commun.garage.fermer');
  function tap() {
    if (!online || busy) return;
    if (moving) act(`${id}/press`, true, t('commun.garage.arreter'));
    else act(`${id}/target`, next, t(verb));
  }
</script>

<section class="card garage {state}" class:off={!online}>
  <div class="stage">
    <span class="scene" aria-hidden="true" style="--pos: {pos}">
      <span class="roof"></span>
      <span class="bay">
        <span class="inside"><span class="lamp"></span><Icon name="car" size={40} /></span>
        <span class="door">
          {#each [0, 1, 2, 3, 4, 5] as slat (slat)}<i></i>{/each}
        </span>
      </span>
    </span>
  </div>

  <div class="bar">
    <span class="text">
      <span class="name">{label}</span>
      <span class="chip" class:pulse={moving}>{online ? word : t('commun.garage.injoignable')}</span>
    </span>
    {#if online}
      <button class="go" class:stop={moving} disabled={busy} aria-label={t(verb)} title={t(verb)} onclick={tap}>
        <Icon name={moving ? 'stop' : next === 'open' ? 'chevron-up' : 'chevron-down'} size={24} />
      </button>
    {/if}
  </div>

  {#if (changed && !moving) || assumed}
    <footer class="muted">
      {#if changed && !moving}<span>{t(`commun.garage.depuis.${state}`, { when: relative(changed, home.now), time: clock(changed) })}</span>{/if}
      {#if assumed}<span class="assumed" title={t('commun.garage.supposee_aide')}><Icon name="info" size={13} />{t('commun.garage.supposee')}</span>{/if}
    </footer>
  {/if}
</section>

<style>
  .garage {
    --tone: var(--good);
    --tone-soft: var(--good-soft);
    display: grid;
    grid-template-rows: 1fr auto;
    align-content: stretch;
    gap: 12px;
    padding: 16px 18px;
    overflow: hidden;
  }

  /* The drawing takes what the tile leaves it, as large as it fits. */
  .stage {
    min-height: 120px;
    container-type: size;
    display: grid;
    place-items: center;
  }

  .garage.open,
  .garage.stopped {
    --tone: var(--warm);
    --tone-soft: var(--warm-soft);
  }

  .garage.opening,
  .garage.closing {
    --tone: var(--cool);
    --tone-soft: var(--cool-soft);
  }

  .off {
    filter: grayscale(0.9);
    opacity: 0.75;
  }

  .bar {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .text {
    flex: 1;
    min-width: 0;
    display: grid;
    justify-items: start;
    gap: 6px;
  }

  .name {
    font-size: 15px;
    font-weight: 650;
    color: var(--ink-2);
  }

  .chip {
    font-size: 12.5px;
    font-weight: 700;
    padding: 4px 10px;
    border-radius: 999px;
    background: var(--tone-soft);
    color: var(--tone);
    white-space: nowrap;
  }

  .garage.open .chip,
  .garage.stopped .chip {
    color: var(--warm-ink);
  }

  .chip.pulse {
    animation: breathe 1.6s ease-in-out infinite;
  }

  .go {
    flex: none;
    width: 52px;
    height: 52px;
    border: 0;
    border-radius: 50%;
    cursor: pointer;
    display: grid;
    place-items: center;
    background: var(--tone);
    color: var(--surface);
    box-shadow: var(--shadow-lift);
    transition: transform 0.15s var(--ease);
  }

  .go.stop {
    background: var(--alert);
    color: #fff;
  }

  .go:active:not(:disabled) {
    transform: scale(0.92);
  }

  .go:disabled {
    opacity: 0.6;
    cursor: default;
  }

  /* A small garage: a roof, a bay, the door sliding up into it. */
  .scene {
    position: relative;
    display: block;
    width: min(100cqw, 320px, 100cqh * 300 / 210);
    aspect-ratio: 300 / 210;
    container-type: inline-size;
  }

  .roof {
    position: absolute;
    inset: 0 0 auto;
    height: 26%;
    background: var(--surface-3);
    clip-path: polygon(50% 0, 100% 78%, 100% 100%, 0 100%, 0 78%);
  }

  .bay {
    position: absolute;
    left: 9%;
    right: 9%;
    top: 26%;
    bottom: 0;
    border-radius: 2cqw 2cqw 0 0;
    background: var(--surface-3);
    /* The door rolls up into the bay, never above it. */
    overflow: hidden;
  }

  .inside {
    position: absolute;
    inset: 2.3cqw 2.3cqw 0;
    border-radius: 1.5cqw 1.5cqw 0 0;
    overflow: hidden;
    display: grid;
    place-items: end center;
    padding-bottom: 4%;
    color: var(--ink-3);
    background: linear-gradient(180deg, color-mix(in oklch, var(--ink) 22%, var(--surface)) 0%, color-mix(in oklch, var(--ink) 10%, var(--surface)) 100%);
  }

  .lamp {
    position: absolute;
    top: 0;
    left: 50%;
    width: 80%;
    height: 80%;
    translate: -50% 0;
    background: radial-gradient(50% 60% at 50% 0%, color-mix(in oklch, var(--warm) calc(var(--pos) * 55%), transparent), transparent);
  }

  .door {
    position: absolute;
    inset: 2.3cqw 2.3cqw 0;
    display: grid;
    grid-template-rows: repeat(6, 1fr);
    gap: 1cqw;
    padding: 1cqw;
    border-radius: 1.5cqw 1.5cqw 0 0;
    background: color-mix(in oklch, var(--ink-3) 35%, var(--surface));
    transform: translateY(calc(var(--pos) * -94%));
    box-shadow: 0 1.5cqw 3cqw -1.5cqw rgb(0 0 0 / 35%);
  }

  .inside :global(svg) {
    width: 22cqw;
    height: auto;
  }

  .door i {
    border-radius: 1cqw;
    background: linear-gradient(180deg, var(--surface) 0%, var(--surface-2) 100%);
  }

  footer {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 4px 12px;
    font-size: 12.5px;
  }

  .assumed {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }

  @keyframes breathe {
    50% {
      opacity: 0.55;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .chip.pulse {
      animation: none;
    }
  }
</style>

<script>
  import { onMount } from 'svelte';
  import { device, value, since, reachable, nameOf, act, pending, relative, clock, home } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** A garage door: drawn, its slats rising and falling at the door's own
   *  pace; slide to open or close (no opening by a stray tap), one tap to
   *  stop it moving. */
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

  // Slide to act: the knob goes to the end of its track.
  let track = $state();
  let drag = $state(null);
  let slid = $state(0);
  function grab(e) {
    if (!online || busy) return;
    drag = { x: e.clientX, w: track.clientWidth - 56 };
    e.currentTarget.setPointerCapture(e.pointerId);
  }
  function slide(e) {
    if (!drag) return;
    slid = Math.max(0, Math.min(1, (e.clientX - drag.x) / drag.w));
  }
  function release() {
    if (!drag) return;
    drag = null;
    if (slid > 0.9) send(next);
    slid = 0;
  }
  function send(where) {
    act(`${id}/target`, where, t(where === 'open' ? 'commun.garage.ouvrir' : 'commun.garage.fermer'));
  }
  const stop = () => act(`${id}/press`, true, t('commun.garage.arreter'));
</script>

<section class="card garage {state}" class:off={!online}>
  <header>
    <h2><Icon name="gate" size={18} />{nameOf(id, name)}</h2>
    <span class="chip" class:pulse={moving}>{word}</span>
  </header>

  <div class="scene" aria-hidden="true" style="--pos: {pos}">
    <div class="roof"></div>
    <div class="bay">
      <div class="inside">
        <div class="lamp"></div>
        <Icon name="car" size={64} />
      </div>
      <div class="door">
        {#each [0, 1, 2, 3, 4, 5] as slat (slat)}<i></i>{/each}
        <b class="handle"></b>
      </div>
    </div>
    <div class="spill"></div>
  </div>

  <div class="controls">
    {#if !online}
      <p class="muted note"><Icon name="offline" size={15} />{t('commun.garage.injoignable')}</p>
    {:else if moving}
      <button class="stop" disabled={busy} onclick={stop}>
        <Icon name="stop" size={18} />{t('commun.garage.arreter')}
      </button>
    {:else}
      <div class="track" class:busy bind:this={track} style="--slid: {slid}">
        <span class="hint">{t(next === 'open' ? 'commun.garage.glisser_ouvrir' : 'commun.garage.glisser_fermer')}</span>
        <button
          class="knob"
          class:dragging={drag}
          aria-label={t(next === 'open' ? 'commun.garage.ouvrir' : 'commun.garage.fermer')}
          disabled={busy}
          onpointerdown={grab}
          onpointermove={slide}
          onpointerup={release}
          onpointercancel={() => ((drag = null), (slid = 0))}
          onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), send(next))}
        >
          <Icon name={next === 'open' ? 'chevron-up' : 'chevron-down'} size={22} />
        </button>
      </div>
    {/if}
  </div>

  <footer class="muted">
    {#if changed && !moving}<span>{t(`commun.garage.depuis.${state}`, { when: relative(changed, home.now), time: clock(changed) })}</span>{/if}
    {#if assumed}<span class="assumed" title={t('commun.garage.supposee_aide')}><Icon name="info" size={13} />{t('commun.garage.supposee')}</span>{/if}
  </footer>
</section>

<style>
  .garage {
    --tone: var(--good);
    --tone-soft: var(--good-soft);
    display: grid;
    gap: 14px;
    overflow: hidden;
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

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
    color: var(--ink-2);
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .chip {
    font-size: 12.5px;
    font-weight: 700;
    padding: 5px 11px;
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

  /* The garage: a roof, a bay, the door sliding up into it. */
  .scene {
    position: relative;
    width: min(100%, 300px);
    margin: 4px auto 0;
    aspect-ratio: 300 / 210;
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
    border-radius: 6px 6px 0 0;
    background: var(--surface-3);
    padding: 7px 7px 0;
    /* The door rolls up into the bay, never above it. */
    overflow: hidden;
  }

  .inside {
    position: absolute;
    inset: 7px 7px 0;
    border-radius: 4px 4px 0 0;
    overflow: hidden;
    display: grid;
    place-items: end center;
    padding-bottom: 6%;
    color: var(--ink-3);
    background: linear-gradient(180deg, color-mix(in oklch, var(--ink) 22%, var(--surface)) 0%, color-mix(in oklch, var(--ink) 10%, var(--surface)) 100%);
  }

  .lamp {
    position: absolute;
    top: 0;
    left: 50%;
    width: 70%;
    height: 80%;
    translate: -50% 0;
    background: radial-gradient(50% 60% at 50% 0%, color-mix(in oklch, var(--warm) calc(var(--pos) * 55%), transparent), transparent);
  }

  .door {
    position: absolute;
    inset: 7px 7px 0;
    display: grid;
    grid-template-rows: repeat(6, 1fr);
    gap: 3px;
    padding: 3px;
    border-radius: 4px 4px 0 0;
    background: color-mix(in oklch, var(--ink-3) 35%, var(--surface));
    transform: translateY(calc(var(--pos) * -94%));
    box-shadow: 0 6px 14px -6px rgb(0 0 0 / 35%);
  }

  .door i {
    border-radius: 3px;
    background: linear-gradient(180deg, var(--surface) 0%, var(--surface-2) 100%);
    box-shadow: inset 0 -2px 0 color-mix(in oklch, var(--ink) 8%, transparent);
  }

  .handle {
    position: absolute;
    left: 50%;
    bottom: 9%;
    width: 18%;
    height: 4%;
    translate: -50% 0;
    border-radius: 3px;
    background: color-mix(in oklch, var(--ink-3) 70%, var(--surface));
  }

  /* Light on the drive when the door is up. */
  .spill {
    position: absolute;
    left: 5%;
    right: 5%;
    bottom: -14%;
    height: 16%;
    background: radial-gradient(50% 100% at 50% 0%, color-mix(in oklch, var(--warm) calc(var(--pos) * 40%), transparent), transparent);
    pointer-events: none;
  }

  .controls {
    min-height: 56px;
    display: grid;
  }

  .track {
    position: relative;
    height: 56px;
    border-radius: 999px;
    background: var(--surface-2);
    overflow: hidden;
  }

  .track::before {
    content: '';
    position: absolute;
    inset: 0;
    width: calc(56px + var(--slid) * (100% - 56px));
    border-radius: 999px;
    background: var(--tone-soft);
  }

  .track.busy {
    opacity: 0.6;
  }

  .hint {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding-left: 40px;
    font-size: 14px;
    font-weight: 650;
    color: var(--ink-3);
    opacity: calc(1 - var(--slid) * 1.4);
    background: linear-gradient(90deg, var(--ink-3) 0%, var(--ink) 50%, var(--ink-3) 100%) 0 0 / 200% 100%;
    -webkit-background-clip: text;
    background-clip: text;
    -webkit-text-fill-color: transparent;
    animation: shine 2.8s linear infinite;
  }

  .knob {
    position: absolute;
    top: 4px;
    left: 4px;
    width: 48px;
    height: 48px;
    border: 0;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--tone);
    color: var(--surface);
    box-shadow: var(--shadow-lift);
    cursor: grab;
    touch-action: none;
    translate: calc(var(--slid) * (100cqw - 56px)) 0;
    transition: translate 0.25s var(--ease);
  }

  .knob.dragging {
    transition: none;
    cursor: grabbing;
  }

  .controls {
    container-type: inline-size;
  }

  .stop {
    height: 56px;
    border: 0;
    border-radius: 999px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    font: inherit;
    font-size: 15px;
    font-weight: 700;
    background: var(--alert);
    color: #fff;
    cursor: pointer;
  }

  .note {
    margin: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    font-size: 13px;
  }

  footer {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 6px 12px;
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

  @keyframes shine {
    to {
      background-position: -200% 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .chip.pulse,
    .hint {
      animation: none;
    }
  }
</style>

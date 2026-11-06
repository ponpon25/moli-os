<script>
  import { onMount, untrack } from 'svelte';
  import { device, value, since, reachable, nameOf, act, isCoverGroup } from '../lib/home.svelte.js';
  import { skyNow } from '../lib/sky.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';
  import ShutterSettings from './ShutterSettings.svelte';
  import CoverGroupEditor from './CoverGroupEditor.svelte';

  /** A roller shutter: how open it is, large, and beside it the window as
   *  a tall pill, onto the sky as it is outside right now, the slats rolling
   *  down over it as the shutter does. The card warms with the light it
   *  lets in. A finger on the window sets where it stops; three buttons
   *  open, stop and close it. `percent_control` is 0 closed, 100 open. */
  let { id, name = null } = $props();

  const d = $derived(device(id));
  const label = $derived(nameOf(id, name));
  const online = $derived(reachable(id));
  const raw = $derived(value(id, 'percent_control'));
  const known = $derived(raw != null);
  // On the sill, the slats pulled apart: light comes through.
  const slatsOpen = $derived(value(id, 'slats_open') === true);
  const pos = $derived(Math.max(0, Math.min(100, Number(raw ?? 0))));
  const offers = $derived(d?.points.find((p) => p.key === 'control')?.kind.values ?? []);
  const settable = $derived(!!d?.points.find((p) => p.key === 'percent_control')?.access?.write);
  // Its durations, for a shutter timed by the driver.
  const tunable = $derived(!!d?.points.find((p) => p.key === 'travel_up')?.access?.write);
  let tuning = $state(false);
  // A group: its settings are its shutters. Some may be out of reach.
  const grouped = $derived(isCoverGroup(d));
  const missing = $derived(grouped ? Number(value(id, 'unreachable') ?? 0) : 0);

  // The sky behind the glass.
  const outside = $derived(skyNow());
  const clear = $derived(outside.scene === 'clear-day' || outside.scene === 'clear-night');
  const wet = $derived(outside.scene === 'rain' || outside.scene === 'storm');

  // Moving: the driver moves the position on every second of the travel;
  // a change seen lately tells the way.
  let way = $state(0);
  let prev = null;
  let still;
  $effect(() => {
    const v = raw;
    void since(id, 'percent_control');
    if (prev != null && v != null && v !== prev) {
      way = Math.sign(v - prev);
      clearTimeout(still);
      still = setTimeout(() => (way = 0), 2200);
    }
    prev = v;
  });
  onMount(() => () => clearTimeout(still));

  // A finger on the window moves a line, where the shutter will stop; the
  // order leaves when it lifts. The slats themselves always follow the
  // position Moli times (one step a second, glided in between), so they
  // travel at the shutter's own pace. What was asked (`aim`) is shown
  // until the shutter has moved and stopped; an order that moves nothing
  // is let go after a few seconds.
  let windowEl = $state();
  let drag = $state(null);
  let aim = $state(null);
  let travelled = false;
  let idle;
  let startY = 0;
  let moved = false;
  const shown = $derived(drag ?? aim ?? pos);
  const target = $derived(drag ?? aim);

  // The number counts along with the slats: from where it is shown to each
  // new position, over the same second the slats take to glide there.
  let live = $state(null);
  let frame;
  $effect(() => {
    const to = pos;
    if (!known) return;
    cancelAnimationFrame(frame);
    const from = untrack(() => live) ?? to;
    const start = performance.now();
    const step = (now) => {
      const k = Math.min(1, (now - start) / GLIDE);
      live = from + (to - from) * k;
      if (k < 1) frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
  });
  onMount(() => () => cancelAnimationFrame(frame));
  const GLIDE = 1100;
  $effect(() => {
    if (aim == null) return;
    if (way !== 0) {
      travelled = true;
      clearTimeout(idle);
    } else if (travelled) {
      aim = null;
    }
  });
  function expect(pct) {
    aim = pct;
    travelled = false;
    clearTimeout(idle);
    idle = setTimeout(() => {
      if (!travelled) aim = null;
    }, 6000);
  }
  onMount(() => () => clearTimeout(idle));

  function at(e) {
    const r = windowEl.getBoundingClientRect();
    const f = Math.max(0, Math.min(1, (e.clientY - r.top) / r.height));
    return Math.round((1 - f) * 100);
  }
  function down(e) {
    if (!online || !settable) return;
    windowEl.setPointerCapture(e.pointerId);
    startY = e.clientY;
    moved = false;
  }
  function move(e) {
    if (!windowEl?.hasPointerCapture(e.pointerId)) return;
    if (!moved && Math.abs(e.clientY - startY) < 6) return;
    moved = true;
    drag = at(e);
  }
  function up(e) {
    if (!windowEl?.hasPointerCapture(e.pointerId)) return;
    windowEl.releasePointerCapture(e.pointerId);
    if (moved && drag != null && e.type === 'pointerup') send(drag);
    drag = null;
  }
  function send(pct) {
    expect(pct);
    act(`${id}/percent_control`, pct, t('commun.action', { label, action: t('commun.volet.ouvert_a', { pct }).toLowerCase() }));
  }

  // The keyboard: arrows move the aim by tens, sent once the keys rest.
  let keyed;
  function key(e) {
    if (!online || !settable) return;
    const step = { ArrowUp: 10, ArrowRight: 10, ArrowDown: -10, ArrowLeft: -10, Home: 100, End: -100 }[e.key];
    if (!step) return;
    e.preventDefault();
    drag = Math.max(0, Math.min(100, Math.round((drag ?? pos) / 10) * 10 + step));
    clearTimeout(keyed);
    keyed = setTimeout(() => {
      send(drag);
      drag = null;
    }, 700);
  }

  function order(cmd, key) {
    if (cmd === 'open') expect(100);
    else if (cmd === 'close') expect(0);
    else aim = null;
    act(`${id}/control`, cmd, t('commun.action', { label, action: t(key).toLowerCase() }));
  }

  const word = $derived.by(() => {
    if (!online) return t('commun.volet.injoignable');
    if (!known && drag == null) return t('commun.volet.inconnue');
    if (way > 0) return t('commun.volet.s_ouvre');
    if (way < 0) return t('commun.volet.se_ferme');
    if (shown >= 99) return t('commun.volet.ouvert');
    if (shown <= 1) return slatsOpen ? t('commun.volet.ajoure') : t('commun.volet.ferme');
    return t('commun.volet.entrouvert');
  });
  const tone = $derived(!online ? 'off' : way !== 0 ? 'moving' : shown <= 1 && !slatsOpen ? 'closed' : 'open');
</script>

{#if tuning}
  {#if grouped}
    <CoverGroupEditor {id} onclose={() => (tuning = false)} />
  {:else}
    <ShutterSettings {id} {label} onclose={() => (tuning = false)} />
  {/if}
{/if}

<section
  class="card shutter {tone} {outside.scene}"
  class:night={!outside.day}
  class:twilight={outside.twilight}
  class:off={!online}
  style="--open: {pos / 100}"
>
  <span class="glow" aria-hidden="true"></span>

  <div class="info">
    <span class="name">{label}</span>
    <span class="pct num" class:unknown={!known && drag == null}>
      {#if drag != null}{drag}<small>%</small>{:else if known}{Math.round(live ?? pos)}<small>%</small>{:else}—{/if}
    </span>
    <span class="state">
      <span class="chip" class:pulse={way !== 0}>{word}</span>
      {#if drag == null && target != null && Math.abs(target - (live ?? pos)) > 1}
        <span class="goal num"><Icon name="chevron-right" size={14} />{target} %</span>
      {/if}
      {#if online && missing > 0}
        <span class="goal">{t('commun.volet.groupe_injoignables', { count: missing })}</span>
      {/if}
    </span>
  </div>

  {#if online}
    <div class="buttons">
      {#if offers.includes('open')}
        <button class:lit={way > 0} aria-label={t('commun.volet.ouvrir')} title={t('commun.volet.ouvrir')} onclick={() => order('open', 'commun.volet.ouvrir')}>
          <Icon name="chevron-up" size={22} />
        </button>
      {/if}
      {#if offers.includes('stop')}
        <button class="stop" class:lit={way !== 0} aria-label={t('commun.volet.stop')} title={t('commun.volet.stop')} onclick={() => order('stop', 'commun.volet.stop')}>
          <Icon name="stop" size={20} />
        </button>
      {/if}
      {#if offers.includes('close')}
        <button class:lit={way < 0} aria-label={t('commun.volet.fermer')} title={t('commun.volet.fermer')} onclick={() => order('close', 'commun.volet.fermer')}>
          <Icon name="chevron-down" size={22} />
        </button>
      {/if}
      {#if tunable || grouped}
        {@const words = t(grouped ? 'commun.volet.groupe_reglages' : 'commun.volet.reglages')}
        <button class="tune" aria-label={words} title={words} onclick={() => (tuning = true)}>
          <Icon name="tune" size={18} />
        </button>
      {/if}
    </div>
  {/if}

  <div
    class="window"
    class:ajoure={slatsOpen && pos <= 1}
    class:grab={online && settable}
    class:dragging={drag != null}
    bind:this={windowEl}
    role="slider"
    tabindex={online && settable ? 0 : -1}
    aria-label={t('commun.volet.aria', { label })}
    aria-valuemin="0"
    aria-valuemax="100"
    aria-valuenow={Math.round(shown)}
    aria-valuetext={word}
    onpointerdown={down}
    onpointermove={move}
    onpointerup={up}
    onpointercancel={up}
    onkeydown={key}
  >
    <span class="air" aria-hidden="true">
      {#if !outside.day && outside.scene !== 'storm'}<span class="stars"></span>{/if}
      {#if outside.day && clear}<span class="sun"></span>{/if}
      {#if !outside.day && clear}<span class="moon"></span>{/if}
      {#if !clear}<span class="cloud c1"></span><span class="cloud c2"></span>{/if}
      <!-- Not `.rain` or `.snow`: those name the weather, on the card itself. -->
      {#if wet}<span class="drops"></span>{/if}
      {#if outside.scene === 'snow'}<span class="flakes"></span>{/if}
    </span>
    <span class="slats" aria-hidden="true"><span class="handle"></span></span>
    {#if target != null && Math.abs(target - pos) > 1}
      <span class="aim" style="--aim: {target / 100}" aria-hidden="true"></span>
    {/if}
  </div>
</section>

<style>
  .shutter {
    --tone: var(--warm);
    --tone-soft: var(--warm-soft);
    --tone-ink: var(--warm-ink);
    /* The light the window lets in: sunlight, gold at twilight, moonlight. */
    --light: 255 205 120;
    --glow: 26%;
    position: relative;
    min-height: 230px;
    padding: 20px 22px;
    overflow: hidden;
    isolation: isolate;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    grid-template-rows: 1fr auto;
    gap: 14px 18px;
  }

  .shutter.twilight {
    --light: 255 160 100;
  }

  .shutter.night {
    --light: 140 170 255;
    --glow: 12%;
  }

  .shutter:is(.overcast-day, .rain, .storm, .fog, .snow):not(.night) {
    --light: 220 228 245;
    --glow: 16%;
  }

  .shutter.moving {
    --tone: var(--cool);
    --tone-soft: var(--cool-soft);
    --tone-ink: var(--cool);
  }

  .shutter.closed {
    --tone: var(--ink-3);
    --tone-soft: var(--surface-2);
    --tone-ink: var(--ink-2);
  }

  .off {
    filter: grayscale(0.9);
    opacity: 0.75;
  }

  /* The card warms with the light coming in, from the window's side. */
  .glow {
    position: absolute;
    inset: 0;
    z-index: -1;
    pointer-events: none;
    background: radial-gradient(90% 85% at 88% 45%, rgb(var(--light) / calc(var(--open) * var(--glow))), transparent 72%);
    transition: background 1.1s linear;
  }

  .info {
    min-width: 0;
    display: grid;
    align-content: start;
    justify-items: start;
    gap: 6px;
  }

  .name {
    font-size: 15px;
    font-weight: 650;
    color: var(--ink-2);
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pct {
    margin-top: 8px;
    font-size: clamp(48px, 9vw, 76px);
    font-weight: 250;
    letter-spacing: -0.04em;
    line-height: 1;
    color: var(--ink);
    font-variant-numeric: tabular-nums;
  }

  .pct small {
    margin-left: 3px;
    font-size: 0.4em;
    letter-spacing: 0;
    color: var(--ink-3);
  }

  .pct.unknown {
    color: var(--ink-3);
  }

  .state {
    margin-top: 6px;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
  }

  .goal {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    font-size: 13px;
    font-weight: 700;
    color: var(--ink-2);
  }

  .chip {
    font-size: 12.5px;
    font-weight: 700;
    padding: 4px 10px;
    border-radius: 999px;
    background: var(--tone-soft);
    color: var(--tone-ink);
    white-space: nowrap;
  }

  .chip.pulse {
    animation: blink 1.6s ease-in-out infinite;
  }

  .buttons {
    grid-row: 2;
    grid-column: 1;
    display: flex;
    gap: 8px;
    align-self: end;
  }

  .buttons button {
    width: 44px;
    height: 44px;
    border: 0;
    border-radius: 50%;
    cursor: pointer;
    display: grid;
    place-items: center;
    background: var(--surface-2);
    color: var(--ink-2);
    transition:
      transform 0.15s var(--ease),
      background 0.2s;
  }

  .buttons button.tune {
    background: transparent;
    opacity: 0.7;
  }

  .buttons button.lit {
    background: var(--tone);
    color: var(--surface);
    box-shadow: var(--shadow-lift);
  }

  .buttons button.stop.lit {
    background: var(--alert);
    color: #fff;
  }

  .buttons button:active {
    transform: scale(0.9);
  }

  /* ---- the window: a tall pill onto the sky ---- */

  .window {
    grid-row: 1 / 3;
    grid-column: 2;
    position: relative;
    width: 88px;
    min-height: 190px;
    border-radius: 32px;
    overflow: hidden;
    background: #0c1220;
    outline: none;
    touch-action: none;
    container-type: size;
    box-shadow:
      inset 0 0 0 1px rgb(255 255 255 / 8%),
      0 10px 30px -12px rgb(0 0 0 / 50%);
  }

  .window.grab {
    cursor: ns-resize;
  }

  .window:focus-visible {
    box-shadow:
      inset 0 0 0 1px rgb(255 255 255 / 8%),
      0 0 0 3px var(--cool);
  }

  /* The slats hang from the top; the bottom one carries a handle. */
  .slats {
    position: absolute;
    inset: 0 0 auto;
    height: calc((1 - var(--open)) * 100%);
    min-height: 8px;
    background:
      linear-gradient(180deg, rgb(255 255 255 / 0%) 70%, rgb(0 0 0 / 10%)),
      repeating-linear-gradient(180deg, #f1f4f8 0 10px, #c6cdd8 10px 11px, #a3abb8 11px 12px);
    box-shadow: 0 6px 14px rgb(0 0 0 / 40%);
    transition: height 1.1s linear;
  }

  .night .slats {
    filter: brightness(0.8);
  }

  /* Pulled apart: the sky shows in a thin line between each slat. */
  .ajoure .slats {
    background:
      linear-gradient(180deg, rgb(255 255 255 / 0%) 70%, rgb(0 0 0 / 10%)),
      repeating-linear-gradient(180deg, #f1f4f8 0 9px, #c6cdd8 9px 10px, transparent 10px 12px);
  }

  /* Where it is going: a line of light across the window. */
  .aim {
    position: absolute;
    left: 8px;
    right: 8px;
    top: calc((1 - var(--aim)) * 100%);
    height: 3px;
    translate: 0 -50%;
    border-radius: 2px;
    background: #fff;
    box-shadow:
      0 0 0 1px rgb(0 0 0 / 15%),
      0 0 10px 2px rgb(255 255 255 / 55%);
    pointer-events: none;
    animation: blink 1.6s ease-in-out infinite;
  }

  .dragging .aim {
    animation: none;
  }

  .handle {
    position: absolute;
    left: 50%;
    bottom: 6px;
    width: 30px;
    height: 5px;
    border-radius: 3px;
    translate: -50% 0;
    background: #7d8696;
  }

  /* ---- the sky (the same colors as the home banner) ---- */

  .air {
    position: absolute;
    inset: 0;
    overflow: hidden;
    transition: background 2s;
  }

  .clear-day .air {
    background:
      radial-gradient(120% 90% at 85% 0%, rgb(255 255 255 / 35%), transparent 55%),
      linear-gradient(170deg, #2a6fd1 0%, #4f9ae9 45%, #9fd0fb 100%);
  }

  .overcast-day .air {
    background: linear-gradient(170deg, #55667e 0%, #7f90a6 50%, #b4bfcc 100%);
  }

  .rain .air {
    background: linear-gradient(170deg, #233247 0%, #3c4d66 50%, #60728b 100%);
  }

  .storm .air {
    background: linear-gradient(170deg, #141a29 0%, #262d45 50%, #45496a 100%);
  }

  .snow .air {
    background: linear-gradient(170deg, #6d83a1 0%, #9fb1c8 50%, #d6e0ec 100%);
  }

  .fog .air {
    background: linear-gradient(170deg, #6f7b88 0%, #9aa5b1 50%, #c3cad2 100%);
  }

  .clear-night .air {
    background:
      radial-gradient(90% 120% at 82% 10%, rgb(70 105 180 / 45%), transparent 60%),
      linear-gradient(170deg, #070f24 0%, #0f1f45 55%, #1b3566 100%);
  }

  .overcast-night .air {
    background: linear-gradient(170deg, #10151f 0%, #1d2535 55%, #2c364a 100%);
  }

  .night:is(.rain, .snow, .fog) .air {
    filter: brightness(0.55) saturate(0.9);
  }

  .twilight:is(.clear-day, .clear-night) .air {
    background:
      radial-gradient(70% 90% at 70% 100%, rgb(255 170 90 / 85%), transparent 65%),
      linear-gradient(180deg, #1f2c5c 0%, #5b4a86 45%, #d9787a 78%, #f6b36b 100%);
  }

  /* Low in the window, so it shows as soon as the shutter opens a little. */
  .sun {
    position: absolute;
    left: 50%;
    top: 70%;
    width: 78cqw;
    aspect-ratio: 1;
    translate: -50% -50%;
    border-radius: 50%;
    background: radial-gradient(circle, #fffdf3 0 18%, #fff1b8 26%, rgb(255 226 140 / 55%) 38%, rgb(255 220 120 / 0%) 70%);
    animation: glow 6s ease-in-out infinite;
  }

  .twilight .sun {
    top: 82%;
    background: radial-gradient(circle, #fff3d6 0 18%, #ffc070 28%, rgb(255 150 80 / 50%) 42%, rgb(255 140 80 / 0%) 70%);
  }

  .moon {
    position: absolute;
    left: 58%;
    top: 66%;
    width: 22cqw;
    aspect-ratio: 1;
    translate: -50% -50%;
    border-radius: 50%;
    background: #f4f1e4;
    box-shadow:
      inset -4px -2px 0 0 #d8d3bf,
      0 0 14px 3px rgb(240 236 210 / 35%);
  }

  .stars {
    position: absolute;
    inset: 0;
    background-image:
      radial-gradient(1px 1px at 22% 18%, #fff, transparent),
      radial-gradient(1px 1px at 70% 30%, rgb(255 255 255 / 70%), transparent),
      radial-gradient(1.4px 1.4px at 40% 46%, #fff, transparent),
      radial-gradient(1px 1px at 18% 62%, rgb(255 255 255 / 60%), transparent),
      radial-gradient(1px 1px at 80% 78%, rgb(255 255 255 / 55%), transparent),
      radial-gradient(1.4px 1.4px at 34% 88%, #fff, transparent);
    animation: twinkle 5s ease-in-out infinite;
  }

  .cloud {
    position: absolute;
    width: 160cqw;
    height: 40cqw;
    border-radius: 50%;
    background: radial-gradient(closest-side, rgb(255 255 255 / 75%), rgb(255 255 255 / 0%));
    animation: drift 30s linear infinite;
  }

  .night .cloud,
  .rain .cloud,
  .storm .cloud {
    background: radial-gradient(closest-side, rgb(60 70 95 / 85%), rgb(60 70 95 / 0%));
  }

  .cloud.c1 {
    top: 52%;
    animation-delay: -8s;
  }

  .cloud.c2 {
    top: 76%;
    animation-duration: 44s;
    animation-delay: -26s;
    opacity: 0.8;
  }

  .drops,
  .flakes {
    position: absolute;
    inset: -20% 0 0;
    background-size: 30cqw 40cqw;
    animation: pour 0.7s linear infinite;
  }

  .drops {
    background-image:
      radial-gradient(0.5px 7px at 30% 20%, rgb(225 238 255 / 60%), transparent),
      radial-gradient(0.5px 5px at 75% 70%, rgb(225 238 255 / 45%), transparent);
  }

  .flakes {
    background-image:
      radial-gradient(1.6px 1.6px at 20% 30%, #fff, transparent),
      radial-gradient(1.2px 1.2px at 70% 80%, rgb(255 255 255 / 85%), transparent);
    animation-duration: 6s;
  }

  @keyframes glow {
    50% {
      scale: 1.08;
    }
  }

  @keyframes twinkle {
    50% {
      opacity: 0.55;
    }
  }

  @keyframes drift {
    from {
      left: -170cqw;
    }
    to {
      left: 110cqw;
    }
  }

  @keyframes pour {
    to {
      translate: 0 40cqw;
    }
  }

  @keyframes blink {
    50% {
      opacity: 0.5;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .slats,
    .glow {
      transition: none;
    }

    .sun,
    .stars,
    .cloud,
    .drops,
    .flakes,
    .aim,
    .chip.pulse {
      animation: none;
    }
  }
</style>

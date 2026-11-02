<script>
  // The top of the home page: the greeting over a living sky, the width of
  // the page. The sky is the weather outside as it is now: blue or grey by
  // day, stars at night, a warm glow at sunrise and sunset, clouds drifting
  // as many as there are, the rain, the snow, a flash in a storm. Only CSS:
  // nothing is drawn when nothing moves, and nothing moves for those who
  // asked for less motion. The weather's words sit on the right, and a
  // vigilance of Météo-France (yellow and above, today or tomorrow) shows as
  // a pill. A tap opens the detail: the map, the hours and the days to come.
  import { onMount } from 'svelte';
  import { home, hub, device, value, clock, greeting, num, longDate } from '../lib/home.svelte.js';
  import { sky, watchSky, hours, outlook, skyNow } from '../lib/sky.svelte.js';
  import { weatherOf } from '../lib/icons.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';
  import SkyDetail from './SkyDetail.svelte';

  const out = $derived(home.config?.outdoor ?? {});
  const outside = $derived(skyNow());
  const w = $derived(outside.w);
  const day = $derived(outside.day);
  const code = $derived(outside.code);
  const cover = $derived(outside.cover);
  const now = $derived(weatherOf(code, day));
  const hhmm = (iso) => (iso ? String(iso).slice(11, 16) : '—');
  const line = $derived(outlook(hours(sky.forecast)));
  let open = $state(false);

  /** What the sky looks like, and the warm glow (`skyNow`). */
  const scene = $derived(outside.scene);
  const clear = $derived(scene === 'clear-day' || scene === 'clear-night');
  const wet = $derived(scene === 'rain' || scene === 'storm');
  const twilight = $derived(outside.twilight);

  /** Clouds: a few when the sky is clear, many when it is not. */
  const clouds = $derived.by(() => {
    const n = clear ? Math.round(Math.min(cover, 80) / 20) : scene === 'fog' ? 2 : 6;
    return Array.from({ length: n }, (_, i) => ({
      top: (i * 37) % 60,
      size: 180 + ((i * 71) % 160),
      dur: 70 + ((i * 29) % 60),
      delay: -((i * 53) % 100),
      opacity: clear ? 0.35 : 0.55 + ((i * 13) % 30) / 100,
    }));
  });

  // The stars: the same every night (a fixed little constellation).
  const STARS = Array.from({ length: 56 }, (_, i) => {
    const x = (i * 61.8) % 100;
    const y = (i * 37.3) % 75;
    const r = i % 7 === 0 ? 1.6 : 1;
    return `radial-gradient(${r}px ${r}px at ${x.toFixed(1)}% ${y.toFixed(1)}%, rgb(255 255 255 / ${i % 3 === 0 ? 95 : 60}%), transparent)`;
  }).join(', ');

  // Météo-France's vigilance: the configured device, or the one there is
  // (known by its points: a level for today and for tomorrow).
  const RANK = { green: 1, yellow: 2, orange: 3, red: 4 };
  const isVigilance = (d) => ['level', 'level_tomorrow', 'summary'].every((k) => d?.points.some((p) => p.key === k));
  const vigilance = $derived(out.vigilance ?? Object.keys(hub.devices).find((id) => isVigilance(hub.devices[id])));
  const alert = $derived.by(() => {
    if (!vigilance || !device(vigilance)) return null;
    const today = value(vigilance, 'level');
    const tomorrow = value(vigilance, 'level_tomorrow');
    if ((RANK[today] ?? 1) >= 2) return { level: today, title: t('maison.meteo.vigilance'), text: value(vigilance, 'summary') };
    if ((RANK[tomorrow] ?? 1) >= 2) return { level: tomorrow, title: t('maison.meteo.vigilance_demain'), text: value(vigilance, 'summary_tomorrow') };
    return null;
  });

  onMount(() => watchSky());
</script>

{#snippet hello()}
  <div class="hello">
    <p class="date">{longDate(home.now)}</p>
    <h1>{greeting(home.now)}</h1>
    <p class="time num">{clock(home.now)}</p>
  </div>
{/snippet}

{#snippet backdrop()}
  <span class="bg" aria-hidden="true">
    <span class="air"></span>
    {#if !day && scene !== 'storm'}<span class="stars" style:background-image={STARS}></span>{/if}
    {#if day && clear}<span class="sun"></span>{/if}
    {#if !day && (clear || scene === 'overcast-night')}<span class="moon" class:veiled={!clear}></span>{/if}
    {#each clouds as c, i (i)}
      <span class="cloud" style:--top="{c.top}%" style:--size="{c.size}px" style:--dur="{c.dur}s" style:--delay="{c.delay}s" style:--o={c.opacity}></span>
    {/each}
    {#if scene === 'fog'}<span class="fog"></span><span class="fog late"></span>{/if}
    {#if wet}<span class="rain"></span><span class="rain mid"></span><span class="rain far"></span>{/if}
    {#if scene === 'snow'}<span class="snow"></span><span class="snow mid"></span><span class="snow far"></span>{/if}
    {#if scene === 'storm'}<span class="flash"></span>{/if}
    <svg class="hills" viewBox="0 0 1200 120" preserveAspectRatio="none">
      <path class="far" d="M0 70 C 140 40 260 52 380 64 S 640 30 780 48 S 1040 70 1200 44 V120 H0 Z" />
      <path class="near" d="M0 96 C 160 72 300 84 460 92 S 760 66 920 82 S 1120 98 1200 86 V120 H0 Z" />
    </svg>
    <span class="scrim"></span>
  </span>
{/snippet}

{#if w}
  <button class="hero {scene}" class:night={!day} class:twilight onclick={() => (open = true)} aria-label={t('maison.meteo.ouvrir', { label: now.label })}>
    {@render backdrop()}
    {@render hello()}
    <span class="weather">
      {#if alert}
        <span class="vigilance {alert.level}" title={alert.text}>
          <Icon name="warning" size={14} />
          <strong>{alert.title}</strong>
          <span class="what">{alert.text}</span>
        </span>
      {/if}
      <span class="now">
        <Icon path={now.icon} size={54} />
        <strong class="temp num">{num(value(w, 'temperature'), 0)}°</strong>
      </span>
      <span class="label">{now.label}</span>
      <span class="small num">
        ↑ {num(value(w, 'today_max'), 0)}° ↓ {num(value(w, 'today_min'), 0)}° ·
        <Icon name={day ? 'sunset' : 'sunrise'} size={14} />
        {day ? hhmm(value(w, 'sunset')) : hhmm(value(w, 'sunrise'))}
      </span>
      <span class="pills num">
        <span>{t('maison.meteo.ressenti', { v: num(value(w, 'feels_like'), 0) })}</span>
        <span>{t('maison.meteo.vent', { v: num(value(w, 'wind_speed'), 0) })}</span>
        <span>{t('maison.meteo.humidite', { v: num(value(w, 'humidity'), 0) })}</span>
      </span>
      {#if line}<span class="small outlook">{line}</span>{/if}
    </span>
  </button>
{:else}
  <section class="hero {scene}" class:night={!day} class:twilight>
    {@render backdrop()}
    {@render hello()}
  </section>
{/if}

{#if open}
  <SkyDetail onclose={() => (open = false)} />
{/if}

<style>
  .hero {
    position: relative;
    isolation: isolate;
    overflow: hidden;
    display: flex;
    justify-content: space-between;
    align-items: end;
    gap: 24px;
    width: 100%;
    /* The tile's height when the family sized it, else its own. */
    height: 100%;
    min-height: 230px;
    padding: 24px 30px;
    border: 0;
    border-radius: var(--r-lg);
    color: #fff;
    text-align: left;
    font: inherit;
    box-shadow: 0 18px 50px -24px rgb(0 0 0 / 55%);
    animation: rise 0.9s var(--ease) both;
  }

  button.hero {
    cursor: pointer;
  }

  button.hero:focus-visible {
    outline: 2px solid #fff;
    outline-offset: 3px;
  }

  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(8px) scale(0.99);
    }
  }

  /* ---- the sky ---- */

  .bg,
  .bg > span {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }

  /* Things placed in the sky (not spread over it). */
  .bg > .sun,
  .bg > .moon,
  .bg > .cloud {
    inset: auto;
  }

  .bg {
    z-index: -1;
  }

  .air {
    transition: background 2s;
  }

  .clear-day .air {
    background:
      radial-gradient(120% 90% at 85% 0%, rgb(255 255 255 / 35%), transparent 55%),
      linear-gradient(165deg, #2a6fd1 0%, #4f9ae9 45%, #9fd0fb 100%);
  }

  .overcast-day .air {
    background: linear-gradient(165deg, #55667e 0%, #7f90a6 50%, #b4bfcc 100%);
  }

  .rain .air {
    background: linear-gradient(165deg, #233247 0%, #3c4d66 50%, #60728b 100%);
  }

  .storm .air {
    background: linear-gradient(165deg, #141a29 0%, #262d45 50%, #45496a 100%);
  }

  .snow .air {
    background: linear-gradient(165deg, #6d83a1 0%, #9fb1c8 50%, #d6e0ec 100%);
  }

  .fog .air {
    background: linear-gradient(165deg, #6f7b88 0%, #9aa5b1 50%, #c3cad2 100%);
  }

  .clear-night .air {
    background:
      radial-gradient(90% 120% at 82% 10%, rgb(70 105 180 / 45%), transparent 60%),
      linear-gradient(165deg, #070f24 0%, #0f1f45 55%, #1b3566 100%);
  }

  .overcast-night .air {
    background: linear-gradient(165deg, #10151f 0%, #1d2535 55%, #2c364a 100%);
  }

  /* Rain and storms at night: darker still. */
  .night.rain .air,
  .night.snow .air,
  .night.fog .air {
    filter: brightness(0.55) saturate(0.9);
  }

  .stars {
    animation: twinkle 5s ease-in-out infinite alternate;
  }

  @keyframes twinkle {
    to {
      opacity: 0.55;
    }
  }

  /* Sunrise and sunset: the sky warms from the horizon up, the sun low. */
  .twilight.clear-day .air,
  .twilight.clear-night .air {
    background:
      radial-gradient(70% 90% at 70% 100%, rgb(255 170 90 / 85%), transparent 65%),
      linear-gradient(180deg, #1f2c5c 0%, #5b4a86 45%, #d9787a 78%, #f6b36b 100%);
  }

  .twilight:is(.overcast-day, .overcast-night) .air {
    background:
      radial-gradient(70% 80% at 70% 100%, rgb(240 150 110 / 55%), transparent 65%),
      linear-gradient(180deg, #2b3044 0%, #5a5670 55%, #a07a7c 100%);
  }

  .twilight .bg > .sun {
    top: auto;
    bottom: -150px;
    background: radial-gradient(circle, #fff3d6 0 26px, #ffc070 36px, rgb(255 150 80 / 50%) 54px, rgb(255 140 80 / 0%) 70%);
  }

  .bg > .sun {
    top: -70px;
    right: 30%;
    width: 240px;
    height: 240px;
    border-radius: 50%;
    background: radial-gradient(circle, #fffdf3 0 22px, #fff1b8 30px, rgb(255 226 140 / 55%) 44px, rgb(255 220 120 / 18%) 70px, rgb(255 220 120 / 0%) 70%);
    filter: blur(1px);
    animation: breathe 8s ease-in-out infinite alternate;
  }

  @keyframes breathe {
    to {
      transform: scale(1.08);
      opacity: 0.9;
    }
  }

  .bg > .moon {
    top: 28px;
    right: 36%;
    width: 46px;
    height: 46px;
    border-radius: 50%;
    box-shadow:
      inset -12px 7px 0 0 #f6f1dc,
      0 0 40px 6px rgb(246 241 220 / 18%);
    transform: rotate(-20deg);
  }

  .moon.veiled {
    opacity: 0.45;
    filter: blur(1.5px);
  }

  .bg > .cloud {
    top: var(--top);
    left: 0;
    width: var(--size);
    height: calc(var(--size) * 0.42);
    border-radius: 50%;
    background: radial-gradient(closest-side, rgb(255 255 255 / 95%), rgb(255 255 255 / 0%));
    filter: blur(14px);
    opacity: var(--o);
    animation: drift var(--dur) linear var(--delay) infinite;
  }

  .night .cloud {
    background: radial-gradient(closest-side, rgb(170 185 210 / 70%), rgb(170 185 210 / 0%));
  }

  .storm .cloud,
  .rain .cloud {
    background: radial-gradient(closest-side, rgb(40 50 70 / 90%), rgb(40 50 70 / 0%));
  }

  @keyframes drift {
    from {
      transform: translateX(-40vw);
    }
    to {
      transform: translateX(110vw);
    }
  }

  /* Three veils of drops, sizes that never line up, so no grid shows. */
  .bg > .rain {
    inset: -200px 0 0 0;
    background-image:
      radial-gradient(0.7px 14px at 30% 20%, rgb(225 238 255 / 55%), transparent),
      radial-gradient(0.7px 10px at 75% 70%, rgb(225 238 255 / 45%), transparent);
    background-size: 53px 131px;
    transform: skewX(-10deg);
    --fall: 131px;
    animation: pour 0.5s linear infinite;
  }

  .bg > .rain.mid {
    background-size: 37px 97px;
    background-position: 11px 23px;
    opacity: 0.7;
    --fall: 97px;
    animation-duration: 0.62s;
  }

  .bg > .rain.far {
    background-size: 23px 71px;
    background-position: 5px 41px;
    opacity: 0.35;
    --fall: 71px;
    animation-duration: 0.8s;
  }

  @keyframes pour {
    to {
      translate: 0 var(--fall);
    }
  }

  .bg > .snow {
    inset: -240px -60px 0 -60px;
    background-image:
      radial-gradient(2.6px 2.6px at 20% 30%, #fff, transparent),
      radial-gradient(2px 2px at 70% 80%, rgb(255 255 255 / 85%), transparent),
      radial-gradient(1.6px 1.6px at 45% 55%, rgb(255 255 255 / 75%), transparent);
    background-size: 113px 157px;
    --fall: 157px;
    --sway: 113px;
    animation: flurry 9s linear infinite;
  }

  .bg > .snow.mid {
    background-size: 71px 97px;
    background-position: 17px 31px;
    opacity: 0.7;
    --fall: 97px;
    --sway: -71px;
    animation-duration: 11s;
  }

  .bg > .snow.far {
    background-size: 41px 59px;
    background-position: 9px 13px;
    opacity: 0.4;
    --fall: 59px;
    --sway: 41px;
    animation-duration: 14s;
  }

  @keyframes flurry {
    to {
      translate: var(--sway) var(--fall);
    }
  }

  .fog {
    background: linear-gradient(180deg, transparent 20%, rgb(255 255 255 / 35%) 45%, transparent 70%);
    animation: haze 26s ease-in-out infinite alternate;
  }

  .fog.late {
    animation-delay: -13s;
    opacity: 0.7;
  }

  @keyframes haze {
    to {
      transform: translate(6%, 12%);
    }
  }

  .flash {
    background: rgb(230 235 255 / 85%);
    opacity: 0;
    animation: flash 9s linear infinite;
  }

  @keyframes flash {
    0%,
    90%,
    93%,
    100% {
      opacity: 0;
    }
    91% {
      opacity: 0.55;
    }
    92% {
      opacity: 0.1;
    }
    92.5% {
      opacity: 0.4;
    }
  }

  /* The land under the sky: two soft hills, darker than it. */
  .hills {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    width: 100%;
    height: 34%;
  }

  .hills .far {
    fill: rgb(10 20 40 / 22%);
  }

  .hills .near {
    fill: rgb(5 12 28 / 38%);
  }

  .night .hills .far {
    fill: rgb(0 0 0 / 35%);
  }

  .night .hills .near {
    fill: rgb(0 0 0 / 55%);
  }

  /* The words stay readable whatever the sky. */
  .scrim {
    background:
      linear-gradient(90deg, rgb(0 0 0 / 30%) 0%, transparent 45%),
      linear-gradient(0deg, rgb(0 0 0 / 28%) 0%, transparent 55%);
  }

  /* ---- the words ---- */

  .hello,
  .weather {
    text-shadow: 0 1px 12px rgb(0 0 0 / 35%);
  }

  .date {
    font-size: 15px;
    font-weight: 600;
    opacity: 0.85;
    text-transform: capitalize;
  }

  h1 {
    font-size: 50px;
    font-weight: 760;
    letter-spacing: -0.035em;
    line-height: 1.05;
  }

  .time {
    font-size: 28px;
    font-weight: 300;
    opacity: 0.9;
  }

  .weather {
    display: grid;
    justify-items: end;
    gap: 4px;
    text-align: right;
  }

  .now {
    display: flex;
    align-items: center;
    gap: 12px;
    color: #ffd75e;
    filter: drop-shadow(0 2px 10px rgb(0 0 0 / 35%));
  }

  .night .now {
    color: #f6f1dc;
  }

  .temp {
    color: #fff;
    font-size: 76px;
    font-weight: 250;
    letter-spacing: -0.04em;
    line-height: 0.95;
  }

  .label {
    font-size: 18px;
    font-weight: 650;
  }

  .small {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 14px;
    font-weight: 550;
    opacity: 0.92;
  }

  .outlook {
    max-width: 280px;
  }

  .pills {
    display: flex;
    flex-wrap: wrap;
    justify-content: end;
    gap: 6px;
    margin: 6px 0 2px;
  }

  .pills > span,
  .vigilance {
    padding: 4px 10px;
    border-radius: 999px;
    background: rgb(255 255 255 / 16%);
    -webkit-backdrop-filter: blur(10px);
    backdrop-filter: blur(10px);
    font-size: 13px;
    font-weight: 600;
    text-shadow: none;
  }

  /* In the top corner, over the sky. */
  .vigilance {
    position: absolute;
    top: 18px;
    right: 22px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: min(460px, 60%);
    color: #1d1d1d;
    font-weight: 550;
    box-shadow: 0 6px 20px -8px rgb(0 0 0 / 50%);
  }

  .vigilance .what {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Météo-France's own colours. */
  .vigilance.yellow {
    background: rgb(249 236 40 / 92%);
  }

  .vigilance.orange {
    background: rgb(247 164 1 / 94%);
  }

  .vigilance.red {
    background: rgb(231 25 25 / 94%);
    color: #fff;
  }

  @media (prefers-reduced-motion: reduce) {
    .hero,
    .bg > span {
      animation: none !important;
    }
  }

  /* A phone: the greeting and the temperature side by side, the rest
     kept to what fits the tile (the pills and the outlook are in the detail). */
  @media (max-width: 760px) {
    .hero {
      gap: 12px;
      min-height: 220px;
      padding: 18px 18px 20px;
    }

    h1 {
      font-size: 32px;
    }

    .date {
      font-size: 13px;
    }

    .time {
      font-size: 20px;
    }

    .temp {
      font-size: 52px;
    }

    .now :global(svg) {
      width: 38px;
      height: 38px;
    }

    .label {
      font-size: 15px;
    }

    .small {
      font-size: 12px;
    }

    .pills,
    .outlook {
      display: none;
    }

    .vigilance {
      left: 16px;
      right: 16px;
      top: 14px;
      max-width: none;
    }

    .bg > .sun {
      right: -60px;
    }

    .bg > .moon {
      right: 30%;
      top: 22px;
    }
  }
</style>

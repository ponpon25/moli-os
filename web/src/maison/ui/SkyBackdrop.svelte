<script>
  // The sky behind the whole dashboard: the weather outside as it is now, blue
  // or grey by day, stars at night, a warm glow at sunrise and sunset, clouds
  // as many as there are, the rain, the snow, a flash in a storm. Only CSS:
  // nothing is drawn when nothing moves, and nothing moves for those who asked
  // for less motion. One sky for every page, so going from one to another
  // changes nothing of it. `calm` (every page but the home page): a veil and
  // no rain or snow across what is being read.
  import { onMount } from 'svelte';
  import { watchSky, skyNow } from '../lib/sky.svelte.js';

  let { calm = false } = $props();

  const outside = $derived(skyNow());
  const day = $derived(outside.day);
  const cover = $derived(outside.cover);
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

  onMount(() => watchSky());
</script>

<div class="sky {scene}" class:night={!day} class:twilight class:calm aria-hidden="true">
  <span class="bg">
  <span class="air"></span>
  {#if !day && scene !== 'storm'}<span class="stars" style:background-image={STARS}></span>{/if}
  {#if day && clear}<span class="sun"></span>{/if}
  {#if !day && (clear || scene === 'overcast-night')}<span class="moon" class:veiled={!clear}></span>{/if}
  {#each clouds as c, i (i)}
    <span class="cloud" style:--top="{c.top}%" style:--size="{c.size}px" style:--dur="{c.dur}s" style:--delay="{c.delay}s" style:--o={c.opacity}></span>
  {/each}
  {#if scene === 'fog'}<span class="fog"></span><span class="fog late"></span>{/if}
  {#if wet && !calm}<span class="rain"></span><span class="rain mid"></span><span class="rain far"></span>{/if}
  {#if scene === 'snow' && !calm}<span class="snow"></span><span class="snow mid"></span><span class="snow far"></span>{/if}
  {#if scene === 'storm' && !calm}<span class="flash"></span>{/if}
  <svg class="hills" viewBox="0 0 1200 120" preserveAspectRatio="none">
    <path class="far" d="M0 70 C 140 40 260 52 380 64 S 640 30 780 48 S 1040 70 1200 44 V120 H0 Z" />
    <path class="near" d="M0 96 C 160 72 300 84 460 92 S 760 66 920 82 S 1120 98 1200 86 V120 H0 Z" />
  </svg>
  <span class="scrim"></span>
    <span class="veil"></span>
</span>
</div>

<style>
  .sky {
    position: fixed;
    inset: 0;
    z-index: -1;
    overflow: hidden;
    pointer-events: none;
  }


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

  .bg > .fog {
    background: linear-gradient(180deg, transparent 20%, rgb(255 255 255 / 35%) 45%, transparent 70%);
    animation: haze 26s ease-in-out infinite alternate;
  }

  .bg > .fog.late {
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
      linear-gradient(180deg, rgb(4 10 24 / 24%) 0%, transparent 26%),
      linear-gradient(0deg, rgb(4 10 24 / 52%) 0%, rgb(4 10 24 / 18%) 52%, transparent 78%);
  }

  .bg > .veil {
    background: #060c1c;
    opacity: 0;
    transition: opacity 0.35s var(--ease);
  }

  .calm .bg > .veil {
    opacity: 0.46;
  }

  @media (prefers-reduced-motion: reduce) {
    .bg > span {
      animation: none !important;
    }
  }

  @media (max-width: 760px) {
    .bg > .sun {
      right: -60px;
    }

    .bg > .moon {
      right: 30%;
      top: 22px;
    }
  }
</style>

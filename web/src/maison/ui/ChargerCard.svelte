<script>
  import { onMount } from 'svelte';
  import { device, value, since, reachable, nameOf, act, pending, clock, home, num, hub, chargerPhase } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** An electric car charger: the wallbox, its cable and the car, the
   *  current flowing down the cable while it charges. The power big, the
   *  session's energy and time, its curve; one button to start or pause, the
   *  current in amps a tap away. */
  let { id, name = null } = $props();

  const d = $derived(device(id));
  // Gradients need an id of their own, of letters and digits.
  const uid = $derived(String(id).replace(/[^a-z0-9]/gi, '-'));
  const has = (key) => d?.points.some((p) => p.key === key) ?? false;
  const online = $derived(reachable(id));

  const phase = $derived(chargerPhase(id));
  const charging = $derived(phase === 'charging');
  const car = $derived(['charging', 'paused', 'plugged'].includes(phase));

  const power = $derived(charging ? Number(value(id, 'cur_power')) || 0 : 0);
  const volts = $derived(Number(value(id, 'cur_voltage')) || 230);
  const energy = $derived(value(id, 'session_energy'));
  const amps = $derived(Number(hub.pending[`${id}/charge_current`] ?? value(id, 'charge_current')) || null);
  const ampPoint = $derived(d?.points.find((p) => p.key === 'charge_current' && p.access?.write));
  const ampMin = $derived(ampPoint?.kind?.min ?? 6);
  const ampMax = $derived(Math.min(ampPoint?.kind?.max ?? 16, Number(value(id, 'max_current')) || Infinity));
  // What the wire can give: the gauge's full scale.
  const top = $derived(Math.max(1000, ampMax * volts));
  const fill = $derived(Math.min(1, power / top));

  // The sessions, from the history: when the last charge began and ended,
  // and its power over time.
  const HOURS = 36;
  let states = $state([]);
  let curve = $state([]);
  async function series(key, points = 600) {
    const res = await fetch(`/api/history?point=${encodeURIComponent(`${id}/${key}`)}&hours=${HOURS}&points=${points}`);
    if (!res.ok) return null;
    return res.json();
  }
  async function load() {
    try {
      const [s, p] = await Promise.all([series('work_state', 2000), has('cur_power') ? series('cur_power') : null]);
      if (s) states = [...(s.before ? [[s.from, s.before[1]]] : []), ...(s.raw ?? [])];
      if (p) curve = (p.raw ?? []).filter(([, v]) => typeof v === 'number');
    } catch {
      /* next round */
    }
  }
  onMount(() => {
    load();
    const timer = setInterval(load, 2 * 60_000);
    return () => clearInterval(timer);
  });
  // A new state: the history has a new chapter.
  $effect(() => {
    void phase;
    const timer = setTimeout(load, 1500);
    return () => clearTimeout(timer);
  });

  const isOn = (v) => ['WORKING', 'CHARGING'].includes(String(v).toUpperCase());
  const session = $derived.by(() => {
    let start = null;
    let end = null;
    for (const [ts, v] of states) {
      if (isOn(v)) {
        if (start === null || end !== null) {
          start = ts;
          end = null;
        }
      } else if (start !== null && end === null) {
        end = ts;
      }
    }
    if (charging && start === null) start = since(id, 'work_state');
    if (start === null) return null;
    // A blip (a few seconds of WORKING while the car wakes) is no session.
    if (!charging && end !== null && end - start < 120_000) return null;
    return { start, end: charging ? null : end };
  });

  const elapsed = $derived(session ? (session.end ?? home.now) - session.start : 0);
  const duration = (ms) => {
    const m = Math.max(0, Math.round(ms / 60_000));
    const h = Math.floor(m / 60);
    return h ? t('commun.borne.duree_h', { h, m: String(m % 60).padStart(2, '0') }) : t('commun.borne.duree_min', { m });
  };

  // The session's power, drawn in a 0..100 box (steps: a value holds).
  const chart = $derived.by(() => {
    if (!session) return null;
    const t0 = session.start;
    const t1 = session.end ?? home.now;
    if (t1 - t0 < 60_000) return null;
    const pts = curve.filter(([ts]) => ts >= t0 && ts <= t1);
    if (charging) pts.push([t1, power]);
    else pts.push([t1, 0]);
    if (pts.length < 2) return null;
    const hi = Math.max(top, ...pts.map(([, v]) => v));
    const x = (ts) => ((ts - t0) / (t1 - t0)) * 100;
    const y = (v) => 100 - (v / hi) * 92;
    let path = `M0 100 L${x(pts[0][0]).toFixed(2)} ${y(pts[0][1]).toFixed(2)}`;
    for (let i = 1; i < pts.length; i++) {
      path += ` L${x(pts[i][0]).toFixed(2)} ${y(pts[i - 1][1]).toFixed(2)} L${x(pts[i][0]).toFixed(2)} ${y(pts[i][1]).toFixed(2)}`;
    }
    return { line: path.replace(/^M0 100 L/, 'M'), area: `${path} L100 100 Z` };
  });

  // About 17 kWh for 100 km: what the session gave back, in words a driver reads.
  const km = $derived(typeof energy === 'number' && energy >= 0.5 ? Math.round((energy / 17) * 100) : null);

  const label = $derived(nameOf(id, name));
  const kw = $derived(num(power / 1000, power >= 10_000 ? 0 : 1));
  const word = $derived(t(`commun.borne.etat.${phase}`));
  const busy = $derived(pending(id, 'charge_switch'));
  const canToggle = $derived(online && car && has('charge_switch'));
  function toggle() {
    if (!canToggle || busy) return;
    act(`${id}/charge_switch`, !charging, t(charging ? 'commun.borne.pause' : 'commun.borne.demarrer'));
  }
  function setAmps(a) {
    const v = Math.max(ampMin, Math.min(ampMax, a));
    if (v !== amps) act(`${id}/charge_current`, v, t('commun.borne.courant'));
  }
</script>

<section class="card charger {phase}">
  <header class="head">
    <span class="title">
      <span class="name">{label}</span>
      <span class="chip" class:live={charging}><i></i>{word}</span>
    </span>
    {#if canToggle}
      <button class="go" class:pause={charging} disabled={busy} aria-label={t(charging ? 'commun.borne.pause' : 'commun.borne.demarrer')} title={t(charging ? 'commun.borne.pause' : 'commun.borne.demarrer')} onclick={toggle}>
        <Icon name={charging ? 'pause' : 'play'} size={22} />
      </button>
    {/if}
  </header>

  <div class="stage" aria-hidden="true">
    <svg viewBox="0 0 320 132" class="scene">
      <defs>
        <linearGradient id="body-{uid}" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" class="body-top" />
          <stop offset="1" class="body-bottom" />
        </linearGradient>
        <radialGradient id="halo-{uid}">
          <stop offset="0" class="halo-in" />
          <stop offset="1" class="halo-out" />
        </radialGradient>
      </defs>

      <ellipse class="halo" cx="215" cy="92" rx="120" ry="46" fill="url(#halo-{uid})" />
      <line class="ground" x1="6" y1="120" x2="314" y2="120" />

      <!-- The wallbox on its wall, its ring telling the state. -->
      <rect class="wall" x="6" y="14" width="6" height="106" rx="2" />
      <rect class="box" x="14" y="30" width="38" height="56" rx="10" />
      <circle class="ring" cx="33" cy="52" r="9" />
      <circle class="ring-core" cx="33" cy="52" r="3.4" />
      <rect class="slot" x="25" y="72" width="16" height="3" rx="1.5" />

      {#if car}
        <!-- The cable to the car's port, the current running down it. -->
        <path class="cable" d="M33 86 C33 128, 110 128, 142 82" />
        <path class="flow" d="M33 86 C33 128, 110 128, 142 82" />
      {:else}
        <path class="cable" d="M33 86 C33 108, 58 110, 56 74" />
        <rect class="holster" x="52" y="68" width="8" height="9" rx="2" />
      {/if}

      <g class="car" class:away={!car}>
        <path class="shadow" d="M136 121 Q220 128 310 121 Q220 116 136 121 Z" />
        <path class="body" fill="url(#body-{uid})" d="M138 104 L138 88 Q138 79 148 77 L178 72 Q196 52 222 50 L258 50 Q278 51 292 68 L304 72 Q314 76 314 88 L314 104 Q314 108 309 108 L138 108 Q138 108 138 104 Z" />
        <path class="glass" d="M186 72 Q200 58 220 56 L242 56 L242 72 Z" />
        <path class="glass" d="M248 56 L258 56 Q274 57 286 71 L248 72 Z" />
        <line class="seam" x1="245" y1="56" x2="245" y2="104" />
        <rect class="light-back" x="139" y="84" width="6" height="7" rx="2" />
        <rect class="light-front" x="306" y="80" width="7" height="5" rx="2" />
        <circle class="port" cx="144" cy="82" r="4" />
        <g class="wheel"><circle cx="168" cy="108" r="14" /><circle class="rim" cx="168" cy="108" r="6" /></g>
        <g class="wheel"><circle cx="284" cy="108" r="14" /><circle class="rim" cx="284" cy="108" r="6" /></g>
        <path class="bolt" d="M232 64 L222 82 L230 82 L226 96 L240 76 L232 76 L236 64 Z" />
      </g>
    </svg>
  </div>

  <div class="figures">
    <div class="power">
      <span class="big num">{charging ? kw : '0'}<small>kW</small></span>
      <span class="gauge"><i style="--fill: {fill}"></i></span>
    </div>
    <dl>
      <div>
        <dt>{t(session?.end && !charging ? 'commun.borne.derniere' : 'commun.borne.session')}</dt>
        <dd class="num">{typeof energy === 'number' ? num(energy, 1) : '—'}<small>kWh</small></dd>
        {#if km}<dd class="sub">{t('commun.borne.km', { km })}</dd>{/if}
      </div>
      <div>
        <dt>{t('commun.borne.duree')}</dt>
        <dd class="num">{session ? duration(elapsed) : '—'}</dd>
        {#if session}
          <dd class="sub">{session.end ? t('commun.borne.de_a', { from: clock(session.start), to: clock(session.end) }) : t('commun.borne.depuis', { time: clock(session.start) })}</dd>
        {/if}
      </div>
    </dl>
  </div>

  {#if chart}
    <svg class="curve" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
      <path class="area" d={chart.area} />
      <path class="line" d={chart.line} vector-effect="non-scaling-stroke" />
    </svg>
  {/if}

  {#if ampPoint && online}
    <div class="amps">
      <span class="muted">{t('commun.borne.courant')}</span>
      <div class="stepper">
        <button disabled={!amps || amps <= ampMin || pending(id, 'charge_current')} aria-label={t('commun.borne.moins')} onclick={() => setAmps((amps ?? ampMax) - 1)}>−</button>
        <span class="num"><b>{amps ?? '—'}</b> A<small>≈ {num(((amps ?? 0) * volts) / 1000, 1)} kW</small></span>
        <button disabled={!amps || amps >= ampMax || pending(id, 'charge_current')} aria-label={t('commun.borne.plus')} onclick={() => setAmps((amps ?? ampMin) + 1)}>+</button>
      </div>
    </div>
  {/if}
</section>

<style>
  .charger {
    --tone: var(--ink-3);
    --tone-soft: var(--surface-3);
    --volt: #22c79a;
    --volt-glow: rgb(34 199 154 / 45%);
    display: grid;
    gap: 14px;
    padding: 18px 18px 16px;
    overflow: hidden;
    isolation: isolate;
  }

  .charging {
    --tone: var(--volt);
    --tone-soft: color-mix(in oklch, var(--volt) 16%, var(--surface));
  }

  .plugged {
    --tone: var(--cool);
    --tone-soft: var(--cool-soft);
  }

  .paused {
    --tone: var(--warm);
    --tone-soft: var(--warm-soft);
  }

  .fault {
    --tone: var(--alert);
    --tone-soft: var(--alert-soft);
  }

  .off {
    filter: grayscale(0.9);
    opacity: 0.75;
  }

  /* While it charges, a slow aurora behind everything. */
  .charging::before {
    content: '';
    position: absolute;
    inset: -40% -20% auto;
    height: 120%;
    z-index: -1;
    background:
      radial-gradient(40% 50% at 70% 40%, color-mix(in oklch, var(--volt) 22%, transparent), transparent 70%),
      radial-gradient(30% 40% at 20% 30%, color-mix(in oklch, var(--cool) 14%, transparent), transparent 70%);
    animation: aurora 9s ease-in-out infinite alternate;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .title {
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
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-size: 12.5px;
    font-weight: 700;
    padding: 4px 10px 4px 8px;
    border-radius: 999px;
    background: var(--tone-soft);
    color: color-mix(in oklch, var(--tone) 70%, var(--ink));
    white-space: nowrap;
  }

  .chip i {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--tone);
  }

  .chip.live i {
    box-shadow: 0 0 0 0 var(--volt-glow);
    animation: ping 1.8s ease-out infinite;
  }

  .go {
    flex: none;
    width: 48px;
    height: 48px;
    border: 0;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--volt);
    color: #fff;
    box-shadow: 0 10px 24px -10px var(--volt-glow), var(--shadow-lift);
    transition: transform 0.15s var(--ease);
  }

  .go.pause {
    background: var(--surface);
    color: var(--ink);
    box-shadow: inset 0 0 0 2px var(--line), var(--shadow-lift);
  }

  .go:active:not(:disabled) {
    transform: scale(0.92);
  }

  .go:disabled {
    opacity: 0.6;
  }

  /* The scene. */
  .stage {
    margin: -4px -6px 0;
  }

  .scene {
    display: block;
    width: 100%;
    height: auto;
    max-height: 170px;
    overflow: visible;
  }

  .halo-in {
    stop-color: var(--tone);
    stop-opacity: 0.22;
  }

  .halo-out {
    stop-color: var(--tone);
    stop-opacity: 0;
  }

  .free .halo,
  .off .halo {
    display: none;
  }

  .charging .halo {
    animation: breathe 3s ease-in-out infinite;
    transform-origin: 215px 92px;
  }

  .ground {
    stroke: var(--line);
    stroke-width: 2;
    stroke-linecap: round;
  }

  .wall {
    fill: var(--surface-3);
  }

  .box {
    fill: var(--surface);
    stroke: color-mix(in oklch, var(--ink) 14%, transparent);
    stroke-width: 1.5;
    filter: drop-shadow(0 4px 6px rgb(0 0 0 / 12%));
  }

  .ring {
    fill: none;
    stroke: var(--tone);
    stroke-width: 3;
  }

  .ring-core {
    fill: var(--tone);
  }

  .charging .ring {
    filter: drop-shadow(0 0 4px var(--volt));
    animation: glow 1.6s ease-in-out infinite;
  }

  .slot {
    fill: var(--surface-3);
  }

  .cable {
    fill: none;
    stroke: color-mix(in oklch, var(--ink) 75%, var(--surface));
    stroke-width: 4.5;
    stroke-linecap: round;
  }

  .holster {
    fill: color-mix(in oklch, var(--ink) 70%, var(--surface));
  }

  .flow {
    fill: none;
    stroke: var(--volt);
    stroke-width: 3;
    stroke-linecap: round;
    stroke-dasharray: 0.1 14;
    opacity: 0;
  }

  .charging .flow {
    opacity: 1;
    filter: drop-shadow(0 0 3px var(--volt));
    animation: run 0.9s linear infinite;
  }

  .car {
    transition: opacity 0.6s var(--ease);
  }

  .car.away {
    opacity: 0.18;
  }

  .shadow {
    fill: rgb(0 0 0 / 18%);
  }

  /* The car's own paint and tyres, whatever the theme does to the ink. */
  .charger {
    --paint-top: #dfe3ea;
    --paint-bottom: #a9b1bf;
    --tyre: #1d2129;
    --rim: #b8c0cc;
  }

  :global(.maison[data-theme='night']) .charger {
    --paint-top: #6a7488;
    --paint-bottom: #3b4253;
    --tyre: #07080b;
    --rim: #8a93a3;
  }

  .body-top {
    stop-color: var(--paint-top);
  }

  .body-bottom {
    stop-color: var(--paint-bottom);
  }

  .glass {
    fill: color-mix(in oklch, var(--cool) 30%, var(--ink) 55%);
    opacity: 0.85;
  }

  .seam {
    stroke: rgb(0 0 0 / 12%);
    stroke-width: 1;
  }

  .light-back {
    fill: var(--alert);
    opacity: 0.8;
  }

  .light-front {
    fill: var(--sun);
    opacity: 0.9;
  }

  .wheel circle {
    fill: var(--tyre);
  }

  .wheel .rim {
    fill: var(--rim);
  }

  .port {
    fill: var(--tone);
  }

  .charging .port {
    filter: drop-shadow(0 0 4px var(--volt));
  }

  .bolt {
    fill: var(--volt);
    opacity: 0;
    transform-origin: 231px 80px;
  }

  .charging .bolt {
    animation: bolt 2.4s ease-in-out infinite;
  }

  /* The figures. */
  .figures {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 18px;
    align-items: end;
  }

  .power {
    display: grid;
    gap: 8px;
  }

  .big {
    font-size: 44px;
    font-weight: 750;
    line-height: 0.95;
    letter-spacing: -0.03em;
    color: var(--ink);
  }

  .charging .big {
    background: linear-gradient(120deg, var(--volt), color-mix(in oklch, var(--volt) 55%, var(--cool)));
    background-clip: text;
    color: transparent;
  }

  small {
    font-size: 0.42em;
    font-weight: 650;
    margin-left: 3px;
    letter-spacing: 0;
    color: var(--ink-3);
  }

  .gauge {
    display: block;
    height: 6px;
    border-radius: 3px;
    background: var(--surface-3);
    overflow: hidden;
  }

  .gauge i {
    display: block;
    height: 100%;
    width: calc(var(--fill) * 100%);
    border-radius: inherit;
    background: linear-gradient(90deg, color-mix(in oklch, var(--volt) 60%, var(--cool)), var(--volt));
    transition: width 0.8s var(--ease);
  }

  dl {
    margin: 0;
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
    justify-items: end;
    text-align: right;
  }

  dt {
    font-size: 12px;
    font-weight: 650;
    color: var(--ink-3);
  }

  dd {
    margin: 2px 0 0;
    font-size: 20px;
    font-weight: 700;
    white-space: nowrap;
  }

  dd small {
    font-size: 0.6em;
  }

  dd.sub {
    font-size: 12px;
    font-weight: 550;
    color: var(--ink-3);
  }

  .curve {
    display: block;
    width: 100%;
    height: 46px;
    margin-top: -4px;
  }

  .curve .area {
    fill: color-mix(in oklch, var(--tone) 16%, transparent);
  }

  .curve .line {
    fill: none;
    stroke: var(--tone);
    stroke-width: 1.8;
    stroke-linejoin: round;
  }

  .free .curve .area,
  .plugged .curve .area {
    fill: var(--surface-3);
  }

  .free .curve .line,
  .plugged .curve .line {
    stroke: var(--ink-3);
  }

  .amps {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding-top: 12px;
    border-top: 1px solid var(--line);
    font-size: 13.5px;
  }

  .amps > span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .stepper {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .stepper button {
    width: 34px;
    height: 34px;
    border: 0;
    border-radius: 50%;
    background: var(--surface-3);
    font-size: 18px;
    font-weight: 700;
    line-height: 1;
  }

  .stepper button:disabled {
    opacity: 0.4;
  }

  .stepper span {
    min-width: 92px;
    text-align: center;
    font-size: 14px;
  }

  .stepper b {
    font-size: 18px;
  }

  .stepper small {
    display: block;
    margin: 0;
    font-size: 11px;
  }

  @keyframes run {
    to {
      stroke-dashoffset: -14.1;
    }
  }

  @keyframes ping {
    0% {
      box-shadow: 0 0 0 0 var(--volt-glow);
    }
    100% {
      box-shadow: 0 0 0 9px transparent;
    }
  }

  @keyframes glow {
    50% {
      stroke-opacity: 0.45;
    }
  }

  @keyframes breathe {
    50% {
      opacity: 0.6;
      transform: scale(1.04);
    }
  }

  @keyframes bolt {
    0%,
    100% {
      opacity: 0;
      transform: scale(0.8);
    }
    35%,
    65% {
      opacity: 0.95;
      transform: scale(1);
    }
  }

  @keyframes aurora {
    to {
      transform: translateX(-8%) translateY(6%);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .charging::before,
    .charging .halo,
    .charging .ring,
    .charging .flow,
    .chip.live i {
      animation: none;
    }

    .charging .bolt {
      animation: none;
      opacity: 0.9;
    }
  }
</style>

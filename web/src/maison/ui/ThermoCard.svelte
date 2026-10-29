<script>
  import { onMount } from 'svelte';
  import { device, value, since, reachable, nameOf, lowBattery, relative, clock, home, num } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** A thermometer (and hygrometer): the temperature big, tinted from cold
   *  blue to hot coral, its last 24 hours as a curve, where it heads, the
   *  day's low and high, the air's humidity as a ring. The battery and the
   *  signal only speak when they have something to say. */
  let { id, name = null } = $props();

  const HOURS = 24;
  const d = $derived(device(id));
  const has = (key) => d?.points.some((p) => p.key === key) ?? false;
  const temp = $derived(value(id, 'temperature'));
  const hum = $derived(has('humidity') ? value(id, 'humidity') : null);
  const pressure = $derived(has('pressure') ? value(id, 'pressure') : null);
  const online = $derived(reachable(id));
  const battery = $derived(lowBattery(id));

  // The history keeps changes only: a value holds until the next one.
  let series = $state([]);
  async function load() {
    if (!has('temperature')) return;
    try {
      const res = await fetch(`/api/history?point=${encodeURIComponent(`${id}/temperature`)}&hours=${HOURS}&points=120`);
      if (!res.ok) return;
      const h = await res.json();
      const points = h.buckets?.length
        ? h.buckets.map((b) => [b.ts, b.avg])
        : (h.raw ?? []).filter(([, v]) => typeof v === 'number');
      if (typeof h.before?.[1] === 'number') points.unshift([h.from, h.before[1]]);
      series = points;
    } catch {
      /* next round */
    }
  }
  // When it last spoke: the hub dates each reading, even an unchanged one
  // (which the event stream does not repeat), so it is asked every minute.
  let heard = $state(0);
  async function ping() {
    try {
      const res = await fetch(`/api/devices/${encodeURIComponent(id)}`);
      if (!res.ok) return;
      const v = await res.json();
      heard = Math.max(0, ...Object.values(v.state ?? {}).map((s) => s.ts ?? 0));
    } catch {
      /* next round */
    }
  }
  onMount(() => {
    load();
    ping();
    const timer = setInterval(load, 5 * 60_000);
    const pinger = setInterval(ping, 60_000);
    return () => (clearInterval(timer), clearInterval(pinger));
  });

  // What is drawn: the history, then the live value up to now.
  const line = $derived.by(() => {
    const pts = series.filter(([ts]) => ts >= home.now - HOURS * 3_600_000);
    if (typeof temp === 'number') pts.push([home.now, temp]);
    return pts;
  });

  // A young sensor: the curve spans what there is (ten minutes at least).
  const start = $derived(Math.min(home.now - 10 * 60_000, line[0]?.[0] ?? home.now));
  const full = $derived(home.now - start >= (HOURS - 0.5) * 3_600_000);

  const extremes = $derived.by(() => {
    if (line.length < 2) return null;
    let lo = line[0];
    let hi = line[0];
    for (const p of line) {
      if (p[1] < lo[1]) lo = p;
      if (p[1] > hi[1]) hi = p;
    }
    return { lo, hi };
  });

  // Where it heads: now against an hour ago.
  const trend = $derived.by(() => {
    if (typeof temp !== 'number' || line.length < 2) return null;
    const ago = home.now - 3_600_000;
    const before = [...line].reverse().find(([ts]) => ts <= ago) ?? line[0];
    if (before[0] > ago + 20 * 60_000) return null;
    const delta = temp - before[1];
    if (Math.abs(delta) < 0.3) return { dir: 'stable', delta };
    return { dir: delta > 0 ? 'hausse' : 'baisse', delta };
  });

  // Cold blue → teal → comfort green → amber → coral, as an oklch hue.
  const STOPS = [
    [-5, 262],
    [8, 238],
    [15, 205],
    [19, 170],
    [22, 135],
    [25, 75],
    [28, 45],
    [33, 25],
  ];
  function hueOf(deg) {
    if (typeof deg !== 'number') return 240;
    if (deg <= STOPS[0][0]) return STOPS[0][1];
    for (let i = 1; i < STOPS.length; i++) {
      const [t1, h1] = STOPS[i];
      const [t0, h0] = STOPS[i - 1];
      if (deg <= t1) return h0 + ((deg - t0) / (t1 - t0)) * (h1 - h0);
    }
    return STOPS.at(-1)[1];
  }
  const hue = $derived(Math.round(hueOf(temp)));

  // 23,4 → « 23 » and « ,4 »: the decimal smaller.
  const parts = $derived.by(() => {
    if (typeof temp !== 'number') return null;
    const s = num(temp, 1);
    const m = s.match(/^(.*?)([.,]\d+)$/);
    return m ? { whole: m[1], dec: m[2] } : { whole: s, dec: '' };
  });

  // The curve: monotone (never overshoots a low or a high), in a 0..100 box.
  const W = 100;
  const H = 100;
  const chart = $derived.by(() => {
    if (line.length < 2) return null;
    const t0 = start;
    const vs = line.map(([, v]) => v);
    let lo = Math.min(...vs);
    let hi = Math.max(...vs);
    if (hi - lo < 2) {
      const mid = (hi + lo) / 2;
      lo = mid - 1;
      hi = mid + 1;
    }
    const pad = (hi - lo) * 0.18;
    lo -= pad;
    hi += pad;
    const x = (ts) => Math.max(0, ((ts - t0) / (home.now - t0)) * W);
    const y = (v) => H - ((v - lo) / (hi - lo)) * H;
    const p = line.map(([ts, v]) => [x(ts), y(v)]);
    const path = monotone(p);
    return {
      path,
      area: `${path} L${W} ${H} L${p[0][0].toFixed(2)} ${H} Z`,
      at: (pt) => ({ left: x(pt[0]), top: y(pt[1]) }),
    };
  });

  function monotone(p) {
    const n = p.length;
    const dx = [];
    const m = [];
    for (let i = 0; i < n - 1; i++) {
      dx.push(p[i + 1][0] - p[i][0] || 1e-6);
      m.push((p[i + 1][1] - p[i][1]) / dx[i]);
    }
    const tan = [m[0]];
    for (let i = 1; i < n - 1; i++) tan.push(m[i - 1] * m[i] <= 0 ? 0 : (m[i - 1] + m[i]) / 2);
    tan.push(m[n - 2]);
    for (let i = 0; i < n - 1; i++) {
      if (m[i] === 0) {
        tan[i] = 0;
        tan[i + 1] = 0;
        continue;
      }
      const a = tan[i] / m[i];
      const b = tan[i + 1] / m[i];
      const s = a * a + b * b;
      if (s > 9) {
        const k = 3 / Math.sqrt(s);
        tan[i] = k * a * m[i];
        tan[i + 1] = k * b * m[i];
      }
    }
    let out = `M${p[0][0].toFixed(2)} ${p[0][1].toFixed(2)}`;
    for (let i = 0; i < n - 1; i++) {
      const h = dx[i] / 3;
      out += ` C${(p[i][0] + h).toFixed(2)} ${(p[i][1] + tan[i] * h).toFixed(2)} ${(p[i + 1][0] - h).toFixed(2)} ${(p[i + 1][1] - tan[i + 1] * h).toFixed(2)} ${p[i + 1][0].toFixed(2)} ${p[i + 1][1].toFixed(2)}`;
    }
    return out;
  }

  // The air: dry, comfortable, damp.
  const air = $derived.by(() => {
    if (typeof hum !== 'number') return null;
    if (hum < 30) return { word: 'tres_sec', tone: 'dry' };
    if (hum < 40) return { word: 'sec', tone: 'dry' };
    if (hum <= 60) return { word: 'confort', tone: 'ok' };
    if (hum <= 70) return { word: 'humide', tone: 'damp' };
    return { word: 'tres_humide', tone: 'damp' };
  });
  const RING = 2 * Math.PI * 15;

  // Signal as four bars, whatever the scale (0-15 for 433 MHz, 0-255 Zigbee).
  const bars = $derived.by(() => {
    const p = d?.points.find((q) => q.key === 'rssi' || q.key === 'linkquality');
    const v = p ? value(id, p.key) : null;
    if (typeof v !== 'number') return null;
    const top = p.kind?.max ?? (p.key === 'rssi' ? 15 : 255);
    return Math.max(1, Math.min(4, Math.round((v / top) * 4)));
  });

  // An SVG id: no colon nor slash.
  const gradient = $derived(`thermo-${id.replace(/[^a-z0-9_-]/gi, '_')}`);

  const last = $derived(Math.max(heard, ...(d?.points ?? []).map((p) => since(id, p.key) ?? 0)));
</script>

<section class="card thermo" class:off={!online} style="--h: {hue}">
  <div class="glow" aria-hidden="true"></div>
  <header>
    <h2><Icon name="thermometer" size={18} />{nameOf(id, name)}</h2>
    <div class="status">
      {#if battery}<span class="pill alert"><Icon name="battery-low" size={14} />{t('commun.thermo.pile')}</span>{/if}
      {#if !online}
        <span class="pill"><Icon name="offline" size={14} />{t('commun.thermo.hors_ligne', { when: relative(last, home.now) })}</span>
      {:else}
        {#if last}<span class="ago" title={t('commun.thermo.releve', { time: clock(last) })}>{relative(last, home.now)}</span>{/if}
        {#if bars}
        <span class="bars" title={t('commun.thermo.signal', { n: bars })} aria-label={t('commun.thermo.signal', { n: bars })}>
          {#each [1, 2, 3, 4] as b (b)}<i class:lit={b <= bars} style="height: {b * 25}%"></i>{/each}
        </span>
        {/if}
      {/if}
    </div>
  </header>

  <div class="main">
    {#if parts}
      <div class="reading">
        <span class="big num">{parts.whole}<small>{parts.dec}</small><sup>°</sup></span>
        {#if trend}
          <span class="trend {trend.dir}">
            {#if trend.dir !== 'stable'}<Icon name={trend.dir === 'hausse' ? 'chevron-up' : 'chevron-down'} size={14} />{/if}
            {t(`commun.thermo.${trend.dir}`)}{#if trend.dir !== 'stable'}<span class="num">&nbsp;{trend.delta > 0 ? '+' : '−'}{num(Math.abs(trend.delta), 1)}° {t('commun.thermo.en_une_heure')}</span>{/if}
          </span>
        {/if}
      </div>
    {:else if typeof hum !== 'number'}
      <p class="muted wait">{t('commun.thermo.attente')}</p>
    {/if}

    {#if typeof hum === 'number'}
      <div class="air {air.tone}" class:alone={!parts}>
        <svg viewBox="0 0 36 36" aria-hidden="true">
          <circle cx="18" cy="18" r="15" class="track" />
          <circle cx="18" cy="18" r="15" class="fill" stroke-dasharray="{(Math.min(100, hum) / 100) * RING} {RING}" />
        </svg>
        <span class="ring-in"><Icon name="humidity" size={14} /><b class="num">{num(hum, 0)}<small>%</small></b></span>
        <span class="air-word">{t(`commun.thermo.${air.word}`)}</span>
      </div>
    {/if}
  </div>

  {#if chart}
    <figure class="chart" aria-label={t('commun.thermo.courbe')}>
      <div class="plot">
      <svg viewBox="0 0 {W} {H}" preserveAspectRatio="none" aria-hidden="true">
        <defs>
          <linearGradient id={gradient} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" class="stop-a" />
            <stop offset="100%" class="stop-b" />
          </linearGradient>
        </defs>
        <path d={chart.area} fill="url(#{gradient})" />
        <path d={chart.path} class="curve" pathLength="1" />
      </svg>
      {#if extremes && extremes.hi[1] - extremes.lo[1] >= 0.3}
        {@const lo = chart.at(extremes.lo)}
        {@const hi = chart.at(extremes.hi)}
        <i class="mark" style="left: {hi.left}%; top: {hi.top}%"></i>
        <i class="mark" style="left: {lo.left}%; top: {lo.top}%"></i>
      {/if}
      {#if online && typeof temp === 'number'}
        {@const now = chart.at([home.now, temp])}
        <i class="now" style="left: {now.left}%; top: {now.top}%"></i>
      {/if}
      </div>
    </figure>
    <footer class="num">
      {#if extremes}
        <span><Icon name="chevron-down" size={13} />{num(extremes.lo[1], 1)}° <em>{clock(extremes.lo[0])}</em></span>
        <span><Icon name="chevron-up" size={13} />{num(extremes.hi[1], 1)}° <em>{clock(extremes.hi[0])}</em></span>
      {/if}
      {#if typeof pressure === 'number'}<span>{num(pressure, 0)} hPa</span>{/if}
      <span class="span">{full ? t('commun.thermo.vingt_quatre') : t('commun.thermo.depuis', { time: clock(start) })}</span>
    </footer>
  {/if}
</section>

<style>
  .thermo {
    --tone: oklch(0.64 0.15 var(--h));
    --tone-ink: oklch(0.42 0.12 var(--h));
    container-type: inline-size;
    overflow: hidden;
    isolation: isolate;
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-height: 100%;
    transition: filter 0.4s var(--ease);
  }

  :global(.maison[data-theme='night']) .thermo {
    --tone: oklch(0.74 0.14 var(--h));
    --tone-ink: oklch(0.86 0.09 var(--h));
  }

  .off {
    filter: grayscale(0.85);
  }

  .off .big {
    opacity: 0.55;
  }

  .glow {
    position: absolute;
    inset: 0;
    z-index: -1;
    background:
      radial-gradient(90% 75% at 100% 0%, color-mix(in oklch, var(--tone) 26%, transparent), transparent 70%),
      radial-gradient(70% 60% at 0% 100%, color-mix(in oklch, var(--tone) 12%, transparent), transparent 70%);
    transition: background 1.2s var(--ease);
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
    min-width: 0;
  }

  h2 :global(svg) {
    color: var(--tone);
    flex: none;
  }

  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }

  .pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 4px 9px;
    border-radius: 999px;
    font-size: 12px;
    font-weight: 650;
    background: var(--surface-2);
    color: var(--ink-3);
  }

  .pill.alert {
    background: var(--alert-soft);
    color: var(--alert);
  }

  .ago {
    font-size: 12px;
    font-weight: 500;
    color: var(--ink-3);
    white-space: nowrap;
  }

  .bars {
    display: inline-flex;
    align-items: flex-end;
    gap: 2px;
    height: 12px;
  }

  .bars i {
    width: 3px;
    border-radius: 2px;
    background: color-mix(in oklch, var(--ink-3) 30%, transparent);
  }

  .bars i.lit {
    background: var(--ink-3);
  }

  .main {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-top: 4px;
  }

  .reading {
    display: grid;
    gap: 2px;
  }

  .big {
    font-size: clamp(52px, 19cqi, 76px);
    font-weight: 700;
    line-height: 0.95;
    letter-spacing: -0.045em;
    color: var(--tone-ink);
    transition: color 1.2s var(--ease);
  }

  .big small {
    font-size: 0.48em;
    letter-spacing: -0.02em;
  }

  .big sup {
    font-size: 0.42em;
    font-weight: 600;
    vertical-align: 0.62em;
    margin-left: -0.02em;
    color: var(--tone);
  }

  .trend {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    font-size: 13px;
    font-weight: 650;
    color: var(--ink-3);
  }

  .trend.hausse {
    color: var(--warm-ink);
  }

  .trend.baisse {
    color: var(--cool);
  }

  .trend .num {
    font-weight: 550;
    color: var(--ink-3);
  }

  .wait {
    margin: 10px 0;
  }

  .air {
    --air: var(--good);
    position: relative;
    display: grid;
    justify-items: center;
    gap: 4px;
    flex: none;
  }

  .air.dry {
    --air: var(--warm);
  }

  .air.damp {
    --air: var(--cool);
  }

  .air.alone {
    margin-left: auto;
    margin-right: auto;
  }

  .air svg {
    width: 64px;
    height: 64px;
    transform: rotate(-90deg);
  }

  .track {
    fill: none;
    stroke: var(--surface-3);
    stroke-width: 3.2;
  }

  .fill {
    fill: none;
    stroke: var(--air);
    stroke-width: 3.2;
    stroke-linecap: round;
    transition: stroke-dasharray 1s var(--ease);
  }

  .ring-in {
    position: absolute;
    top: 0;
    width: 64px;
    height: 64px;
    display: grid;
    place-items: center;
    align-content: center;
    gap: 0;
    color: var(--air);
  }

  .ring-in b {
    font-size: 15px;
    font-weight: 700;
    color: var(--ink);
    line-height: 1;
  }

  .ring-in small {
    font-size: 10px;
    font-weight: 650;
    color: var(--ink-3);
  }

  .air-word {
    font-size: 12px;
    font-weight: 650;
    color: var(--ink-3);
    white-space: nowrap;
  }

  .chart {
    position: relative;
    margin: 6px -20px 0;
    height: 86px;
    flex: 1 0 86px;
    max-height: 240px;
  }

  /* Room on the right for the « now » dot and its pulse. */
  .plot {
    position: absolute;
    inset: 10px 18px 0 6px;
  }

  .chart svg {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    overflow: visible;
  }

  .stop-a {
    stop-color: var(--tone);
    stop-opacity: 0.32;
  }

  .stop-b {
    stop-color: var(--tone);
    stop-opacity: 0;
  }

  .curve {
    fill: none;
    stroke: var(--tone);
    stroke-width: 2.4;
    stroke-linecap: round;
    vector-effect: non-scaling-stroke;
    stroke-dasharray: 1;
    stroke-dashoffset: 1;
    animation: draw 1.4s var(--ease) 0.1s forwards;
  }

  .mark,
  .now {
    position: absolute;
    width: 7px;
    height: 7px;
    margin: -3.5px 0 0 -3.5px;
    border-radius: 50%;
    background: var(--surface);
    box-shadow: 0 0 0 2px var(--tone);
  }

  .now {
    width: 10px;
    height: 10px;
    margin: -5px 0 0 -5px;
    left: 100%;
    background: var(--tone);
    box-shadow: 0 0 0 3px var(--surface);
  }

  .now::after {
    content: '';
    position: absolute;
    inset: -3px;
    border-radius: 50%;
    border: 2px solid var(--tone);
    animation: pulse 2.4s var(--ease) infinite;
  }

  footer {
    display: flex;
    align-items: center;
    gap: 14px;
    font-size: 12px;
    font-weight: 650;
    color: var(--ink-2);
  }

  footer span {
    display: inline-flex;
    align-items: center;
    gap: 2px;
  }

  footer em {
    font-style: normal;
    font-weight: 500;
    color: var(--ink-3);
    margin-left: 3px;
  }

  footer .span {
    margin-left: auto;
    color: var(--ink-3);
    font-weight: 500;
  }

  @keyframes draw {
    to {
      stroke-dashoffset: 0;
    }
  }

  @keyframes pulse {
    0% {
      transform: scale(1);
      opacity: 0.8;
    }
    70%,
    100% {
      transform: scale(2.4);
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .curve {
      animation: none;
      stroke-dashoffset: 0;
    }

    .now::after {
      animation: none;
      display: none;
    }
  }

  @container (max-width: 250px) {
    .air-word,
    footer em,
    footer .span {
      display: none;
    }

    .air svg,
    .ring-in {
      width: 54px;
      height: 54px;
    }
  }
</style>

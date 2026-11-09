<script>
  /** A point's last hours as the thermometer card draws them: a smooth line
   *  over a tinted fill, its low and high marked, the live value pulsing at
   *  the right, the reading under the pointer, and the low and high written
   *  below. For any number (a power, a temperature…). */
  import { onMount } from 'svelte';
  import { clock, home, num } from '../lib/home.svelte.js';
  import { t, locale } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  let {
    point,
    hours = 24,
    /** The live value, drawn up to now. */
    live = null,
    unit = '',
    digits = 0,
    /** Hue of the tint (oklch degrees). */
    hue = 60,
    /** The scale starts at zero (a power), not at the lowest reading. */
    zero = false,
    /** Below this spread between low and high, the marks are left out. */
    minSpread = 0,
    /** Each value holds until the next one (a power jumps; a temperature drifts). */
    step = false,
    label = '',
    /** What a full span is called (« dernières 24 h »). */
    spanLabel = '',
    online = true,
  } = $props();

  // The history keeps changes only: a value holds until the next one.
  let series = $state([]);
  async function load() {
    try {
      const res = await fetch(`/api/history?point=${encodeURIComponent(point)}&hours=${hours}&points=120`);
      if (!res.ok) return;
      const h = await res.json();
      const pts = h.buckets?.length ? h.buckets.map((b) => [b.ts, b.avg]) : (h.raw ?? []).filter(([, v]) => typeof v === 'number');
      if (typeof h.before?.[1] === 'number') pts.unshift([h.from, h.before[1]]);
      series = pts;
    } catch {
      /* next round */
    }
  }
  onMount(() => {
    load();
    const timer = setInterval(load, 5 * 60_000);
    return () => clearInterval(timer);
  });

  const span = $derived(hours * 3_600_000);
  const line = $derived.by(() => {
    const pts = series.filter(([ts, v]) => ts >= home.now - span && typeof v === 'number');
    if (typeof live === 'number') pts.push([home.now, live]);
    return pts;
  });
  // A young point: the curve spans what there is (ten minutes at least).
  const start = $derived(Math.min(home.now - 10 * 60_000, line[0]?.[0] ?? home.now));
  const full = $derived(home.now - start >= span - 30 * 60_000);

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

  const W = 100;
  const H = 100;
  const chart = $derived.by(() => {
    if (line.length < 2) return null;
    const t0 = start;
    const vs = line.map(([, v]) => v);
    let lo = zero ? Math.min(0, ...vs) : Math.min(...vs);
    let hi = Math.max(...vs);
    const least = zero ? 1 : 2;
    if (hi - lo < least) {
      if (zero) hi = lo + least;
      else {
        const mid = (hi + lo) / 2;
        lo = mid - 1;
        hi = mid + 1;
      }
    }
    const pad = (hi - lo) * 0.18;
    if (!zero) lo -= pad;
    hi += pad;
    const x = (ts) => Math.max(0, ((ts - t0) / (home.now - t0)) * W);
    const y = (v) => H - ((v - lo) / (hi - lo)) * H;
    const held = step ? line.flatMap((pt, i) => (i && pt[0] - line[i - 1][0] > 60_000 ? [[pt[0] - 60_000, line[i - 1][1]], pt] : [pt])) : line;
    const p = held.map(([ts, v]) => [x(ts), y(v)]);
    const path = monotone(p);
    return {
      path,
      area: `${path} L${W} ${H} L${p[0][0].toFixed(2)} ${H} Z`,
      at: (pt) => ({ left: x(pt[0]), top: y(pt[1]) }),
      when: (f) => t0 + f * (home.now - t0),
    };
  });

  /** A moment, as short as it can be said: « 14:05 », « hier 14:05 », « mar. 14:05 ». */
  function moment(ts) {
    const day = new Date(ts).toDateString();
    if (day === new Date(home.now).toDateString()) return clock(ts);
    if (day === new Date(home.now - 86_400_000).toDateString()) return t('commun.thermo.hier', { time: clock(ts) });
    return `${new Date(ts).toLocaleDateString(locale(), { weekday: 'short' })} ${clock(ts)}`;
  }

  // Hovering the curve (a mouse or a pen): the reading nearest the pointer.
  let hover = $state(null);
  function aim(e) {
    if (e.pointerType === 'touch' || !chart) return;
    const r = e.currentTarget.getBoundingClientRect();
    const ts = chart.when(Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)));
    let best = line[0];
    for (const pt of line) if (Math.abs(pt[0] - ts) < Math.abs(best[0] - ts)) best = pt;
    hover = best;
  }
  const tip = $derived.by(() => {
    if (!hover || !chart) return null;
    const pos = chart.at(hover);
    return {
      ...pos,
      value: `${num(hover[1], digits)}${unit}`,
      time: hover[0] === home.now ? t('commun.thermo.long.maintenant') : moment(hover[0]),
      side: pos.left < 15 ? 'start' : pos.left > 85 ? 'end' : 'mid',
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

  // An SVG id: no colon nor slash.
  const gradient = $derived(`curve-${point.replace(/[^a-z0-9_-]/gi, '_')}-${hours}`);
  const spanText = $derived(full ? spanLabel : t('commun.thermo.depuis', { time: moment(start) }));
</script>

<div class="hc" style="--h: {hue}">
  {#if chart}
    <figure class="chart" aria-label={label}>
      <div class="plot" role="presentation" onpointermove={aim} onpointerleave={() => (hover = null)}>
        <svg viewBox="0 0 {W} {H}" preserveAspectRatio="none" aria-hidden="true">
          <defs>
            <linearGradient id={gradient} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" class="stop-a" />
              <stop offset="100%" class="stop-b" />
            </linearGradient>
          </defs>
          <path d={chart.area} fill="url(#{gradient})" />
          <path d={chart.path} class="curve" />
        </svg>
        {#if extremes && extremes.hi[1] - extremes.lo[1] > minSpread}
          {@const lo = chart.at(extremes.lo)}
          {@const hi = chart.at(extremes.hi)}
          <i class="mark" style="left: {hi.left}%; top: {hi.top}%"></i>
          <i class="mark" style="left: {lo.left}%; top: {lo.top}%"></i>
        {/if}
        {#if online && typeof live === 'number'}
          {@const now = chart.at([home.now, live])}
          <i class="now" style="left: {now.left}%; top: {now.top}%"></i>
        {/if}
        {#if tip}
          <i class="guide" style="left: {tip.left}%"></i>
          <i class="probe" style="left: {tip.left}%; top: {tip.top}%"></i>
          <span class="tip {tip.side}" style="left: {tip.left}%; top: {tip.top}%"><b>{tip.value}</b><em>{tip.time}</em></span>
        {/if}
      </div>
    </figure>
    <footer class="num">
      {#if extremes}
        <span><Icon name="chevron-down" size={13} />{num(extremes.lo[1], digits)}{unit} <em>{moment(extremes.lo[0])}</em></span>
        <span><Icon name="chevron-up" size={13} />{num(extremes.hi[1], digits)}{unit} <em>{moment(extremes.hi[0])}</em></span>
      {/if}
      <span class="span">{spanText}</span>
    </footer>
  {:else}
    <p class="empty">{t('maison.courbe.vide')}</p>
  {/if}
</div>

<style>
  .hc {
    --tone: oklch(0.7 0.15 var(--h));
    --tone-ink: oklch(0.45 0.12 var(--h));
    container-type: inline-size;
    display: grid;
    gap: 6px;
  }

  :global(.maison[data-theme='night']) .hc {
    --tone: oklch(0.78 0.14 var(--h));
    --tone-ink: oklch(0.88 0.09 var(--h));
  }

  .chart {
    position: relative;
    margin: 4px 0 0;
    height: 96px;
  }

  /* Room on the right for the « now » dot and its pulse. */
  .plot {
    position: absolute;
    inset: 10px 14px 0 2px;
    cursor: crosshair;
  }

  .chart svg {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    overflow: visible;
    clip-path: inset(-12px 100% -12px -12px);
    animation: draw 1.4s var(--ease) 0.1s forwards;
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
  }

  .guide {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 0;
    border-left: 1px dashed color-mix(in oklch, var(--tone) 55%, transparent);
    pointer-events: none;
  }

  .probe {
    position: absolute;
    width: 9px;
    height: 9px;
    margin: -4.5px 0 0 -4.5px;
    border-radius: 50%;
    background: var(--tone);
    box-shadow: 0 0 0 2.5px var(--surface);
    pointer-events: none;
    z-index: 2;
  }

  .tip {
    position: absolute;
    z-index: 3;
    transform: translate(-50%, calc(-100% - 10px));
    display: flex;
    align-items: baseline;
    gap: 6px;
    padding: 4px 8px;
    border-radius: 8px;
    background: var(--surface);
    box-shadow: 0 2px 10px rgb(0 0 0 / 0.14), 0 0 0 1px color-mix(in oklch, var(--tone) 30%, transparent);
    font-size: 12px;
    white-space: nowrap;
    pointer-events: none;
    font-variant-numeric: tabular-nums;
  }

  .tip.start {
    transform: translate(-10px, calc(-100% - 10px));
  }

  .tip.end {
    transform: translate(calc(-100% + 10px), calc(-100% - 10px));
  }

  .tip b {
    font-weight: 700;
    color: var(--tone-ink);
  }

  .tip em {
    font-style: normal;
    font-weight: 500;
    color: var(--ink-3);
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
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 14px;
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

  .empty {
    margin: 0;
    font-size: 12px;
    color: var(--ink-3);
  }

  @keyframes draw {
    to {
      clip-path: inset(-12px -12px -12px -12px);
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
    .chart svg {
      animation: none;
      clip-path: none;
    }

    .now::after {
      animation: none;
      display: none;
    }
  }

  @container (max-width: 250px) {
    footer em,
    footer .span {
      display: none;
    }
  }
</style>

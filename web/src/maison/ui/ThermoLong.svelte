<script>
  import { onMount, untrack } from 'svelte';
  import { device, value, nameOf, home, num } from '../lib/home.svelte.js';
  import { hueOf, humidityHue, midnight, rows, ticks } from '../lib/thermo.js';
  import { t, locale } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** A thermometer over the long run: one bar per day (or per month), from
   *  its low to its high, coloured by what it says, its average as a dot;
   *  the records and the mean of the period beside. */
  let { id, name = null, onclose } = $props();

  const d = $derived(device(id));
  const has = (key) => d?.points.some((p) => p.key === key) ?? false;
  // `days` back for days, `months` back (this one included) for months.
  const SPANS = [
    { key: 'semaine', days: 7, by: 'day' },
    { key: 'mois', days: 30, by: 'day' },
    { key: 'annee', months: 12, by: 'month' },
    { key: 'tout', months: 120, by: 'month' },
  ];
  let spanKey = $state('semaine');
  const span = $derived(SPANS.find((s) => s.key === spanKey));
  let metric = $state('temperature');
  let series = $state(null);
  let loading = $state(false);
  /** The bar looked at (its day), else the last one. */
  let picked = $state(null);
  let dialog;

  // From a midnight (a first of the month for months), by day.
  const from = $derived.by(() => {
    if (span.by === 'day') return midnight(home.now, span.days - 1);
    const m = new Date(midnight(home.now));
    m.setDate(1);
    m.setMonth(m.getMonth() - (span.months - 1));
    return m.getTime();
  });

  let asked = 0;
  async function load() {
    const mine = ++asked;
    loading = true;
    try {
      const q = new URLSearchParams({
        point: `${id}/${metric}`,
        from: String(from),
        step: '86400000',
        points: '5000',
        hours: String((home.now - from) / 3_600_000),
      });
      const res = await fetch(`/api/history?${q}`);
      const s = res.ok ? await res.json() : null;
      if (mine === asked) series = s;
    } catch {
      if (mine === asked) series = null;
    } finally {
      if (mine === asked) loading = false;
    }
  }
  // Again when the period or the measure changes (and only then).
  $effect(() => {
    void spanKey;
    void metric;
    untrack(() => {
      picked = null;
      load();
    });
  });

  onMount(() => {
    dialog.showModal();
  });

  const list = $derived(rows(series, from, home.now, span.by));
  // « Tout »: from the first month with a reading.
  const shown = $derived.by(() => {
    if (span.key !== 'tout') return list;
    const first = list.findIndex((r) => r.avg != null && !r.carried);
    return first < 0 ? [] : list.slice(first);
  });
  const real = $derived(shown.filter((r) => r.avg != null));
  const empty = $derived(!real.some((r) => !r.carried));

  const scale = $derived.by(() => {
    if (!real.length) return null;
    const lo = Math.min(...real.map((r) => r.min));
    const hi = Math.max(...real.map((r) => r.max));
    const marks = ticks(lo, hi);
    const bottom = Math.min(marks[0], lo);
    const top = Math.max(marks.at(-1), hi);
    return { bottom, top, marks: marks.filter((m) => m >= bottom && m <= top) };
  });
  const pos = (v) => (scale ? ((v - scale.bottom) / Math.max(1e-6, scale.top - scale.bottom)) * 100 : 0);

  const unit = $derived(metric === 'temperature' ? '°' : ' %');
  const digits = $derived(metric === 'temperature' ? 1 : 0);
  const color = (v) =>
    metric === 'temperature' ? `oklch(var(--l) var(--c) ${hueOf(v)})` : `oklch(var(--l) var(--c) ${humidityHue(v)})`;

  const records = $derived.by(() => {
    const days = real.filter((r) => !r.carried);
    if (!days.length) return null;
    const lo = days.reduce((a, b) => (b.min < a.min ? b : a));
    const hi = days.reduce((a, b) => (b.max > a.max ? b : a));
    const mean = days.reduce((s, r) => s + r.avg, 0) / days.length;
    return { lo, hi, mean };
  });

  const current = $derived(shown.find((r) => r.ts === picked) ?? shown.at(-1) ?? null);
  const live = $derived(value(id, metric));

  const fmtDay = (ts, opts) => new Date(ts).toLocaleDateString(locale(), opts);
  const when = (ts) =>
    span.by === 'day' ? fmtDay(ts, { weekday: 'long', day: 'numeric', month: 'long' }) : fmtDay(ts, { month: 'long', year: 'numeric' });
  function label(r, i) {
    if (span.key === 'semaine') return fmtDay(r.ts, { weekday: 'short' }).replace('.', '');
    if (span.by === 'day') return i % 5 === shown.length % 5 - 1 || i === shown.length - 1 ? fmtDay(r.ts, { day: 'numeric' }) : '';
    // Initials (J F M A…): twelve fit on a phone.
    const every = shown.length > 24 ? 3 : 1;
    return (shown.length - 1 - i) % every === 0 ? fmtDay(r.ts, { month: shown.length > 6 ? 'narrow' : 'short' }).replace('.', '') : '';
  }
  const isToday = (r) => span.by === 'day' && r.ts === midnight(home.now);
</script>

<dialog bind:this={dialog} {onclose} onclick={(e) => e.target === dialog && dialog.close()} aria-label={t('commun.thermo.long.titre', { name: nameOf(id, name) })}>
  <header>
    <div>
      <p class="eyebrow">{t('commun.thermo.long.historique')}</p>
      <h2>{nameOf(id, name)}</h2>
    </div>
    <button class="close" onclick={() => dialog.close()} aria-label={t('commun.thermo.long.fermer')}><Icon name="close" size={22} /></button>
  </header>

  <div class="bar">
    <div class="seg" role="radiogroup" aria-label={t('commun.thermo.long.periode')}>
      {#each SPANS as s (s.key)}
        <button role="radio" aria-checked={spanKey === s.key} class:on={spanKey === s.key} onclick={() => (spanKey = s.key)}>{t(`commun.thermo.long.${s.key}`)}</button>
      {/each}
    </div>
    {#if has('temperature') && has('humidity')}
      <div class="seg" role="radiogroup" aria-label={t('commun.thermo.long.mesure')}>
        <button role="radio" aria-checked={metric === 'temperature'} class:on={metric === 'temperature'} onclick={() => (metric = 'temperature')}>
          <Icon name="thermometer" size={15} />{t('commun.thermo.long.temperature')}
        </button>
        <button role="radio" aria-checked={metric === 'humidity'} class:on={metric === 'humidity'} onclick={() => (metric = 'humidity')}>
          <Icon name="humidity" size={15} />{t('commun.thermo.long.humidite')}
        </button>
      </div>
    {/if}
  </div>

  <div class="focus" aria-live="polite">
    {#if current && current.avg != null}
      <span class="when">{isToday(current) ? t('commun.thermo.long.aujourdhui') : when(current.ts)}</span>
      <span class="range num">
        <b style="color: {color(current.min)}">{num(current.min, digits)}{unit}</b>
        <i aria-hidden="true" style="background: linear-gradient(90deg, {color(current.min)}, {color(current.max)})"></i>
        <b style="color: {color(current.max)}">{num(current.max, digits)}{unit}</b>
      </span>
      <span class="muted avg num">
        {t('commun.thermo.long.moyenne_de', { value: `${num(current.avg, digits)}${unit}` })}
        {#if current.carried}· {t('commun.thermo.long.sans_releve')}{/if}
      </span>
    {:else if current && !empty}
      <span class="when">{when(current.ts)}</span>
      <span class="muted">{t('commun.thermo.long.aucun_releve')}</span>
    {:else if !loading}
      <span class="muted">{t('commun.thermo.long.vide')}</span>
    {/if}
  </div>

  <figure class="chart" class:loading>
    {#if scale && !empty}
      <div class="grid" aria-hidden="true">
        {#each scale.marks as m (m)}
          <span style="bottom: {pos(m)}%"><em class="num">{num(m, 0)}{unit}</em></span>
        {/each}
      </div>
      <div class="cols" style="--n: {shown.length}">
        {#each shown as r, i (r.ts)}
          <button
            class="col"
            class:picked={current === r}
            class:today={isToday(r)}
            class:carried={r.carried}
            onclick={() => (picked = r.ts)}
            onmouseenter={() => (picked = r.ts)}
            aria-label={r.avg == null ? when(r.ts) : `${when(r.ts)} : ${num(r.min, digits)} – ${num(r.max, digits)}${unit}`}
          >
            {#if r.avg != null}
              <span
                class="rng"
                style="bottom: {pos(r.min)}%; height: max(5px, {pos(r.max) - pos(r.min)}%); background: linear-gradient(0deg, {color(r.min)}, {color(r.max)}); animation-delay: {Math.min(i * 12, 500)}ms"
              ></span>
              <span class="dot" style="bottom: {pos(r.avg)}%"></span>
            {/if}
            <span class="x">{label(r, i)}</span>
          </button>
        {/each}
      </div>
    {:else}
      <p class="muted empty">{loading ? '…' : t('commun.thermo.long.vide')}</p>
    {/if}
  </figure>

  {#if records}
    <dl class="stats">
      <div>
        <dt><Icon name="chevron-down" size={14} />{metric === 'temperature' ? t('commun.thermo.long.plus_froid') : t('commun.thermo.long.plus_sec')}</dt>
        <dd class="num" style="color: {color(records.lo.min)}">{num(records.lo.min, digits)}{unit}</dd>
        <dd class="muted">{when(records.lo.ts)}</dd>
      </div>
      <div>
        <dt><Icon name="chevron-up" size={14} />{metric === 'temperature' ? t('commun.thermo.long.plus_chaud') : t('commun.thermo.long.plus_humide')}</dt>
        <dd class="num" style="color: {color(records.hi.max)}">{num(records.hi.max, digits)}{unit}</dd>
        <dd class="muted">{when(records.hi.ts)}</dd>
      </div>
      <div>
        <dt>{t('commun.thermo.long.moyenne')}</dt>
        <dd class="num" style="color: {color(records.mean)}">{num(records.mean, digits)}{unit}</dd>
        <dd class="muted">{t(`commun.thermo.long.sur_${span.key}`)}</dd>
      </div>
      {#if typeof live === 'number'}
        <div>
          <dt>{t('commun.thermo.long.maintenant')}</dt>
          <dd class="num" style="color: {color(live)}">{num(live, digits)}{unit}</dd>
          <dd class="muted">{t('commun.thermo.long.en_direct')}</dd>
        </div>
      {/if}
    </dl>
  {/if}
</dialog>

<style>
  dialog {
    --l: 0.66;
    --c: 0.15;
    border: 0;
    padding: 22px;
    width: min(860px, 96vw);
    max-height: 94dvh;
    overflow-y: auto;
    border-radius: var(--r-xl);
    background: var(--surface);
    color: var(--ink);
    box-shadow: 0 30px 80px rgb(0 0 0 / 40%);
  }

  :global(.maison[data-theme='night']) dialog {
    --l: 0.76;
    --c: 0.14;
  }

  dialog::backdrop {
    background: rgb(4 6 12 / 60%);
    backdrop-filter: blur(4px);
  }

  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }

  .eyebrow {
    margin: 0;
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--ink-3);
  }

  h2 {
    margin: 2px 0 0;
    font-size: 24px;
    font-weight: 700;
    letter-spacing: -0.02em;
  }

  .close {
    border: 0;
    background: var(--surface-2);
    color: var(--ink-2);
    border-radius: 999px;
    width: 40px;
    height: 40px;
    display: grid;
    place-items: center;
    cursor: pointer;
    flex: none;
  }

  .bar {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    justify-content: space-between;
    margin: 18px 0 10px;
  }

  .seg {
    display: inline-flex;
    padding: 3px;
    gap: 2px;
    border-radius: 999px;
    background: var(--surface-2);
  }

  .seg button {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    border: 0;
    background: transparent;
    color: var(--ink-3);
    font: inherit;
    font-size: 13px;
    font-weight: 650;
    padding: 7px 13px;
    border-radius: 999px;
    cursor: pointer;
    transition:
      background 0.2s var(--ease),
      color 0.2s var(--ease);
  }

  .seg button.on {
    background: var(--surface);
    color: var(--ink);
    box-shadow: var(--shadow);
  }

  .focus {
    min-height: 58px;
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 6px 14px;
    padding: 6px 2px 0;
  }

  .when {
    flex-basis: 100%;
    font-size: 13px;
    font-weight: 650;
    color: var(--ink-2);
    text-transform: capitalize;
  }

  .range {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    font-size: 30px;
    font-weight: 700;
    letter-spacing: -0.03em;
  }

  .range i {
    width: 44px;
    height: 6px;
    border-radius: 3px;
  }

  .avg {
    font-size: 13px;
  }

  .chart {
    position: relative;
    margin: 10px 0 0;
    height: clamp(200px, 38vh, 300px);
    padding: 8px 0 26px 34px;
    transition: opacity 0.3s;
  }

  .chart.loading {
    opacity: 0.55;
  }

  .grid {
    position: absolute;
    inset: 8px 0 26px 34px;
    pointer-events: none;
  }

  .grid span {
    position: absolute;
    left: 0;
    right: 0;
    border-top: 1px dashed var(--line);
  }

  .grid em {
    position: absolute;
    right: calc(100% + 6px);
    top: -8px;
    font-style: normal;
    font-size: 11px;
    font-weight: 600;
    color: var(--ink-3);
    white-space: nowrap;
  }

  .cols {
    position: relative;
    height: 100%;
    display: grid;
    grid-template-columns: repeat(var(--n), minmax(0, 1fr));
    gap: clamp(1px, calc(40% / var(--n)), 10px);
  }

  .col {
    position: relative;
    border: 0;
    padding: 0;
    background: transparent;
    border-radius: 8px;
    cursor: pointer;
    font: inherit;
  }

  .col::before {
    content: '';
    position: absolute;
    inset: -4px -2px;
    border-radius: 10px;
    background: var(--surface-2);
    opacity: 0;
    transition: opacity 0.15s;
  }

  .col.picked::before {
    opacity: 1;
  }

  .rng {
    position: absolute;
    left: 18%;
    right: 18%;
    border-radius: 999px;
    transform-origin: bottom;
    animation: rise 0.6s var(--ease) both;
  }

  .carried .rng {
    opacity: 0.35;
  }

  .dot {
    position: absolute;
    left: 50%;
    width: 7px;
    height: 7px;
    margin: 0 0 -3.5px -3.5px;
    border-radius: 50%;
    background: var(--surface);
    box-shadow: 0 0 0 1.5px var(--ink-2);
  }

  .x {
    position: absolute;
    top: calc(100% + 6px);
    left: 50%;
    transform: translateX(-50%);
    font-size: 11px;
    font-weight: 600;
    color: var(--ink-3);
    white-space: nowrap;
    text-transform: capitalize;
  }

  .today .x {
    color: var(--ink);
  }

  .empty {
    height: 100%;
    display: grid;
    place-items: center;
    margin: 0;
  }

  .stats {
    margin: 16px 0 0;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 10px;
  }

  .stats div {
    background: var(--surface-2);
    border-radius: var(--r-md);
    padding: 12px 14px;
  }

  dt {
    display: flex;
    align-items: center;
    gap: 3px;
    font-size: 12px;
    font-weight: 650;
    color: var(--ink-3);
  }

  dd {
    margin: 2px 0 0;
    font-size: 12px;
    text-transform: capitalize;
  }

  dd.num {
    font-size: 24px;
    font-weight: 700;
    letter-spacing: -0.02em;
  }

  @keyframes rise {
    from {
      transform: scaleY(0);
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .rng {
      animation: none;
    }
  }

  @media (max-width: 520px) {
    dialog {
      padding: 16px;
    }

    .range {
      font-size: 24px;
    }

    .seg button {
      padding: 7px 10px;
    }
  }
</style>

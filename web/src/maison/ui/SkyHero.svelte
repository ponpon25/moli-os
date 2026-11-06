<script>
  // Every page's frame, over the sky (`SkyBackdrop`): the time and
  // the weather's words at the top, and a
  // vigilance of Météo-France (yellow and above, today or tomorrow) shows as
  // a pill. A tap opens the detail: the map, the hours and the days to come.
  import { home, hub, device, value, clock, num, longDate } from '../lib/home.svelte.js';
  import { skyNow } from '../lib/sky.svelte.js';
  import { weatherOf } from '../lib/icons.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';
  import SkyDetail from './SkyDetail.svelte';

  let { children } = $props();

  const out = $derived(home.config?.outdoor ?? {});
  const outside = $derived(skyNow());
  const w = $derived(outside.w);
  const day = $derived(outside.day);
  const code = $derived(outside.code);
  const cover = $derived(outside.cover);
  const now = $derived(weatherOf(code, day));
  let open = $state(false);


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

</script>

<section class="hero">
  <header class="top">
    <div class="when"><b class="num">{clock(home.now)}</b><span>{longDate(home.now)}</span></div>
    {#if w}
      <button class="wx" onclick={() => (open = true)} aria-label={t('maison.meteo.ouvrir', { label: now.label })}>
        {#if alert}
          <span class="vigilance {alert.level}" title={alert.text}>
            <Icon name="warning" size={14} />
            <strong>{alert.title}</strong>
          </span>
        {/if}
        <Icon path={now.icon} size={26} />
        <strong class="temp num">{num(value(w, 'temperature'), 0)}°</strong>
        <span class="words">{now.label}<small class="num">{num(value(w, 'today_min'), 0)}° – {num(value(w, 'today_max'), 0)}°</small></span>
      </button>
    {/if}
  </header>
  {@render children?.()}
</section>

{#if open}
  <SkyDetail onclose={() => (open = false)} />
{/if}

<style>
  .hero {
    display: flex;
    flex-direction: column;
    width: 100%;
    min-height: 100dvh;
    color: #fff;
  }

  /* ---- the words ---- */

  .top {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 20px;
    width: 100%;
    max-width: 1320px;
    margin: 0 auto;
    padding: 26px clamp(20px, 5vw, 72px) 0;
    text-shadow: 0 1px 12px rgb(0 0 0 / 35%);
  }

  .when {
    display: grid;
    gap: 2px;
  }

  .when b {
    font-size: 21px;
    font-weight: 650;
  }

  .when span {
    font-size: 14px;
    color: rgb(255 255 255 / 84%);
  }

  .wx {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 6px 10px;
    padding: 4px 8px;
    border: 0;
    border-radius: 14px;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    text-shadow: inherit;
    cursor: pointer;
  }

  .wx:hover {
    background: rgb(255 255 255 / 12%);
  }

  .wx:focus-visible {
    outline: 2px solid #fff;
    outline-offset: 2px;
  }

  .temp {
    font-family: var(--serif);
    font-size: 32px;
    font-weight: 400;
  }

  .words {
    display: grid;
    font-size: 13px;
    line-height: 1.3;
    color: rgb(255 255 255 / 88%);
  }

  .words small {
    font-size: inherit;
  }

  .vigilance {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 5px 11px;
    border-radius: 999px;
    color: #1c1407;
    font-size: 12.5px;
    text-shadow: none;
  }

  .vigilance.yellow {
    background: #ffe16a;
  }

  .vigilance.orange {
    background: #ffae4a;
  }

  .vigilance.red {
    background: #e5482f;
    color: #fff;
  }

  @media (max-width: 760px) {
    .top {
      padding-top: 18px;
    }
  }
</style>

<script>
  /** A shutter, or a group of them: up, stop, down, and its position to the
   *  unit. `tune`: its settings too (a shutter's durations, a group's list). */
  import { act, nameOf, reachable, isCoverGroup } from '../lib/home.svelte.js';
  import { coverOf } from '../lib/accueil.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';
  import LevelSlider from './LevelSlider.svelte';
  import ShutterSettings from './ShutterSettings.svelte';
  import CoverGroupEditor from './CoverGroupEditor.svelte';

  let { d, name, sub = null, tune = false } = $props();

  const c = $derived(coverOf(d));
  const ok = $derived(reachable(d.id));
  const tunable = $derived(tune && (isCoverGroup(d) || d.points.some((p) => p.key === 'travel_up' && p.access?.write)));
  let tuning = $state(false);

  const BUTTONS = [
    { command: 'open', word: 'ouvrir', icon: 'chevron-up', size: 17 },
    { command: 'stop', word: 'stop', icon: 'stop', size: 15 },
    { command: 'close', word: 'fermer', icon: 'chevron-down', size: 17 },
  ];
  const said = (b) => t('commun.action', { label: nameOf(d.id), action: t(`commun.volet.${b.word}`).toLowerCase() });
</script>

<div class="h-line">
  <div class="h-row">
    <span class="h-ico" class:lit={c.open}><Icon name="shutter" size={19} /></span>
    <span class="names">
      <b>{name}</b>
      {#if sub}<em>{sub}</em>{/if}
    </span>
    <small class:spare={ok && c.settable}>{!ok ? t('commun.lampe.injoignable') : c.settable && c.pct > 0 && c.pct < 100 ? '' : c.word}</small>
    <div class="h-mini">
      {#each BUTTONS.filter((b) => c.offers.includes(b.command)) as b (b.command)}
        <button aria-label={said(b)} title={t(`commun.volet.${b.word}`)} disabled={!ok} onclick={() => act(`${d.id}/control`, b.command, said(b))}><Icon name={b.icon} size={b.size} /></button>
      {/each}
      {#if tunable}
        <button aria-label={t('maison.accueil.reglages_de', { name: nameOf(d.id) })} title={t('maison.accueil.reglages_de', { name: nameOf(d.id) })} onclick={() => (tuning = true)}><Icon name="tune" size={16} /></button>
      {/if}
    </div>
  </div>
  {#if c.settable}
    <LevelSlider value={c.pct ?? 0} label={t('maison.accueil.position', { label: name })} disabled={!ok} onchange={(p) => act(`${d.id}/percent_control`, p, nameOf(d.id))} />
  {/if}
</div>

{#if tuning}
  {#if isCoverGroup(d)}
    <CoverGroupEditor id={d.id} onclose={() => (tuning = false)} />
  {:else}
    <ShutterSettings id={d.id} label={nameOf(d.id)} onclose={() => (tuning = false)} />
  {/if}
{/if}

<style>
  .names {
    min-width: 0;
    display: grid;
    line-height: 1.2;
  }

  /* A phone is narrow: the level below says it already. */
  @media (max-width: 760px) {
    small.spare {
      visibility: hidden;
    }
  }

  em {
    color: var(--ink-3);
    font-size: 12.5px;
    font-style: normal;
  }
</style>

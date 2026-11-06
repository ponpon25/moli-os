<script>
  /** A light on one line: its switch and, when it dims, its level to the unit
   *  (moved from 0, it lights the lamp). */
  import { nameOf, setPower } from '../lib/home.svelte.js';
  import { lightOf, setLight } from '../lib/accueil.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';
  import LevelSlider from './LevelSlider.svelte';

  let { d, name } = $props();
  const l = $derived(lightOf(d));
</script>

<div class="h-line">
  <div class="h-row">
    <span class="h-ico" class:lit={l.on}><Icon name="light" size={19} /></span>
    <b>{name}</b>
    <small>{l.on && l.key ? '' : l.word}</small>
    <button class="h-switch" class:on={l.on} role="switch" aria-checked={l.on} aria-label={nameOf(d.id)} disabled={l.cut || !l.ok} onclick={() => setPower(d, !l.on, nameOf(d.id))}><i></i></button>
  </div>
  {#if l.key}
    <LevelSlider value={l.pct} label={t('commun.lampe.luminosite', { label: name })} disabled={l.cut || !l.ok} onchange={(p) => setLight(d, p)} />
  {/if}
</div>

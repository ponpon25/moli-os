<script>
  /** A speaker on one line: what it plays, play or pause, and its volume. */
  import { act, nameOf, reachable } from '../lib/home.svelte.js';
  import { speakerOf } from '../lib/accueil.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';
  import HomeCover from './HomeCover.svelte';
  import LevelSlider from './LevelSlider.svelte';

  /** `name`: a group's rooms. `onopen`: everything else (the full player). */
  let { d, name = null, onopen = null } = $props();
  const s = $derived(speakerOf(d));
</script>

<div class="h-line">
  <div class="h-row">
    {#if s.playing || s.paused}<HomeCover id={d.id} art={s.art} />{:else}<HomeCover id={d.id} />{/if}
    <div class="info">
      <b>{name ?? nameOf(d.id)}</b>
      <small>{s.playing || s.paused ? [s.title, s.artist].filter(Boolean).join(' · ') : t('maison.accueil.rien_en_cours')}</small>
    </div>
    <button class="h-round play" class:on={s.playing} aria-label={s.playing ? t('salon.pause') : t('salon.lecture')} disabled={!reachable(d.id)} onclick={() => act(`${d.id}/playing`, !s.playing, s.playing ? t('salon.pause') : t('salon.lecture'))}><Icon name={s.playing ? 'pause' : 'play'} size={20} /></button>
    {#if onopen}<button class="h-round" aria-label={t('maison.perso.ouvrir', { name: name ?? nameOf(d.id) })} title={t('maison.perso.ouvrir', { name: name ?? nameOf(d.id) })} onclick={onopen}><Icon name="chevron-down" size={18} /></button>{/if}
  </div>
  {#if s.playing}
    <div class="vol">
      <Icon name="volume-high" size={17} />
      <LevelSlider value={s.volume} label={t('salon.volume')} unit="" onchange={(v) => act(`${d.id}/volume`, v, t('salon.volume'))} />
    </div>
  {/if}
</div>

<style>
  .info {
    flex: 1;
    min-width: 0;
    display: grid;
  }

  .info small {
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .play.on {
    background: var(--warm);
    color: #2d2108;
  }

  .vol {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 6px;
    color: var(--ink-3);
  }
</style>

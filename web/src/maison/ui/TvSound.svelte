<script>
  /** Where the Apple TV's sound goes: a switch per AirPlay speaker, then
   *  its own speakers. A speaker turned on takes the TV's place (the driver
   *  removes « Haut-parleurs TV », as one does by hand). */
  import { act, value, reachable } from '../lib/home.svelte.js';
  import { soundSwitches } from '../lib/appletv.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  let { id } = $props();
  const switches = $derived(soundSwitches(id));
  const output = $derived(value(id, 'audio_output'));
  let busy = $state(null);

  async function flip(p) {
    busy = p.key;
    const on = value(id, p.key) === true;
    await act(`${id}/${p.key}`, !on, p.label);
    // The Apple TV tells the new outputs a moment later.
    setTimeout(() => (busy = null), 2500);
  }
</script>

{#if switches.length}
  <section class="card sound">
    <div class="card-head">
      <h2><Icon name="speaker" size={18} />{t('salon.atv.son')}</h2>
      {#if output}<span class="muted now">{output}</span>{/if}
    </div>
    {#each switches as p (p.key)}
      {@const on = value(id, p.key) === true}
      <div class="h-line">
        <div class="h-row">
          <span class="h-ico" class:lit={on}><Icon name={p.key === 'tv_speakers' ? 'tv' : 'speaker'} size={19} /></span>
          <b>{p.label}</b>
          <small>{busy === p.key ? t('salon.tele.un_instant') : on ? t('salon.atv.joue_ici') : ''}</small>
          <button class="h-switch" class:on role="switch" aria-checked={on} aria-label={p.label} disabled={!reachable(id) || busy != null} onclick={() => flip(p)}><i></i></button>
        </div>
      </div>
    {/each}
    {#if value(id, 'audio_output') == null}<p class="muted small">{t('salon.atv.son_inconnu')}</p>{/if}
  </section>
{/if}

<style>
  .sound {
    padding: 22px;
  }

  .card-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 12px;
    margin-bottom: 6px;
  }

  .now {
    font-size: 13px;
    text-align: right;
  }

  .small {
    font-size: 13px;
    margin-top: 8px;
  }
</style>

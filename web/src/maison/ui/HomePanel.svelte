<script>
  // What opens from the home page: a use of the house, or a room. The orders
  // go through the real cards and `act`: the guard and the code still hold.
  import { onMount } from 'svelte';
  import { act, clock, isOn, nameOf, num, roomOf, value, hub } from '../lib/home.svelte.js';
  import { alarms, allLightsOff, cameras, chargers, clims, coverGroups, covers, coversWord, garages, lightOf, lights, rooms, shortName, smokes, speakers } from '../lib/accueil.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';
  import CoverRow from './CoverRow.svelte';
  import LightRow from './LightRow.svelte';
  import RoomDetail from './RoomDetail.svelte';
  import SmokeRow from './SmokeRow.svelte';
  import SpeakerRow from './SpeakerRow.svelte';
  import SwitchRow from './SwitchRow.svelte';
  import AlarmCard from './AlarmCard.svelte';
  import CameraTile from './CameraTile.svelte';
  import ChargerCard from './ChargerCard.svelte';
  import ClimateCard from './ClimateCard.svelte';
  import EnergyCard from './EnergyCard.svelte';
  import GarageCard from './GarageCard.svelte';
  import Routines from './Routines.svelte';
  import CanalChannels from './CanalChannels.svelte';
  import TvSound from './TvSound.svelte';
  import { appleTv, APPLE_APPS } from '../lib/appletv.svelte.js';

  let { panel, onclose } = $props();

  const roomName = $derived(panel.startsWith('room:') ? panel.slice(5) : null);
  const room = $derived(roomName ? rooms().find((r) => r.name === roomName) : null);
  const title = $derived(roomName ?? t(`maison.accueil.usage.${panel}`));

  /** Grouped by room, those in no room last. */
  function byRoom(list) {
    const map = new Map();
    for (const d of list) {
      const r = roomOf(d) ?? '';
      if (!map.has(r)) map.set(r, []);
      map.get(r).push(d);
    }
    return [...map.entries()].sort((a, b) => !a[0] - !b[0] || a[0].localeCompare(b[0]));
  }

  const LINKS = { tv: 'salon', lights: 'pieces', covers: 'stores', music: 'enceintes', security: 'cameras', energy: 'energie' };

  // The TV, through the Apple TV: on or off, a channel, an app, the sound.
  const atv = $derived(appleTv()?.id ?? null);
  const atvOn = $derived(atv ? value(atv, 'power') === true : false);
  const link = $derived(LINKS[panel]);

  // The house's mode (a helper, « Mode Maison ») and the speakers that talk:
  // one tap silences them, for four hours at most.
  const modeDevice = $derived(Object.values(hub.devices).find((d) => d.id.endsWith(':mode_maison')));
  const modes = $derived(modeDevice?.points.find((p) => p.key === 'state')?.kind.values ?? []);
  const mode = $derived(modeDevice ? value(modeDevice.id, 'state') : null);
  const MODE_ICON = { Normal: 'home', Nuit: 'moon', Absent: 'lock', Vacances: 'palm' };
  function modeName(m) {
    const key = `maison.accueil.mode.${String(m).toLowerCase()}`;
    const w = t(key);
    return w === key ? m : w;
  }
  const talkers = $derived(Object.values(hub.devices).filter((d) => d.points.some((p) => p.key === 'quiet' && p.access?.write)));
  const hushed = $derived(talkers.some((d) => value(d.id, 'quiet') === true));
  const hushUntil = $derived(Math.max(0, ...talkers.map((d) => Number(value(d.id, 'quiet_until') ?? 0))));
  function toggleHush() {
    for (const d of talkers) act(`${d.id}/quiet`, !hushed, hushed ? t('maison.accueil.annonces_retablies') : t('maison.accueil.annonces_coupees'));
  }

  let self = $state();
  onMount(() => self?.focus());
</script>

<aside class="panel" role="dialog" aria-modal="true" aria-label={title} tabindex="-1" bind:this={self}>
  <div class="head">
    <h2>{title}</h2>
    <div class="head-right">
      {#if room?.temp != null}
        <span class="heat num">{num(room.temp, 1)}°{#if room.humidity != null}<small>{num(room.humidity)} %</small>{/if}</span>
      {/if}
      <button class="h-round" onclick={onclose} aria-label={t('commun.fermer')}><Icon name="close" size={20} /></button>
    </div>
  </div>

  {#if roomName}
    {#if room}<RoomDetail {room} />{:else}<p class="muted">{t('maison.accueil.piece_vide')}</p>{/if}
  {:else if panel === 'lights'}
    {#if lights().some((d) => isOn(d.id))}
      <button class="wide" onclick={allLightsOff}><Icon name="power" size={18} />{t('maison.accueil.tout_eteindre')}</button>
    {/if}
    {#each byRoom(lights()) as [r, list] (r)}
      <h3>{r || t('maison.accueil.sans_piece')}</h3>
      {#each list as d (d.id)}<LightRow {d} name={shortName(d, r)} />{/each}
    {/each}
  {:else if panel === 'covers'}
    <p class="muted">{coversWord()}</p>
    {#if coverGroups().length}
      <h3>{t('maison.accueil.groupes')}</h3>
      {#each coverGroups() as d (d.id)}<CoverRow {d} name={nameOf(d.id)} />{/each}
    {/if}
    {#each byRoom(covers()) as [r, list] (r)}
      <h3>{r || t('maison.accueil.sans_piece')}</h3>
      {#each list as d (d.id)}<CoverRow {d} name={shortName(d, r)} />{/each}
    {/each}
  {:else if panel === 'climate'}
    <div class="cards">
      {#each clims() as d (d.id)}<ClimateCard id={d.id} name={nameOf(d.id)} />{/each}
    </div>
  {:else if panel === 'tv' && atv}
    <button class="wide" class:on={atvOn} aria-pressed={atvOn} onclick={() => act(`${atv}/power`, !atvOn, atvOn ? t('salon.tele.eteindre') : t('salon.tele.allumer'))}>
      <Icon name="power" size={18} />{atvOn ? t('salon.tele.eteindre_television') : t('salon.tele.allumer_television')}
    </button>
    <CanalChannels id={atv} compact />
    <div class="apps" role="group" aria-label={t('salon.ouvrir_une_appli')}>
      {#each APPLE_APPS as a (a.id)}
        <button onclick={() => act(`${atv}/app`, a.id, t('salon.ouvrir', { app: a.name }))}>{a.name}</button>
      {/each}
    </div>
    <TvSound id={atv} />
  {:else if panel === 'music'}
    {#if talkers.length}
      <button class="wide hush" class:on={hushed} aria-pressed={hushed} title={hushed ? t('maison.accueil.sonos_reparle') : t('maison.accueil.sonos_coupe')} onclick={toggleHush}>
        <Icon name={hushed ? 'volume-off' : 'speaker'} size={18} />{hushed && hushUntil ? t('maison.accueil.sonos_muette_jusqua', { time: clock(hushUntil) }) : t('maison.accueil.sonos_muette')}
      </button>
    {/if}
    {#each speakers() as d (d.id)}<SpeakerRow {d} />{/each}
  {:else if panel === 'security'}
    {#if modes.length}
      <div class="modes" role="radiogroup" aria-label={t('maison.accueil.mode_maison')}>
        {#each modes as m (m)}
          <button role="radio" aria-checked={m === mode} class:on={m === mode} onclick={() => m !== mode && act(`${modeDevice.id}/state`, m, t('maison.accueil.mode_action', { mode: modeName(m) }))}><Icon name={MODE_ICON[m] ?? 'home'} size={16} />{modeName(m)}</button>
        {/each}
      </div>
    {/if}
    {#each alarms() as d (d.id)}<AlarmCard id={d.id} />{/each}
    <div class="cards">
      {#each garages() as d (d.id)}<GarageCard id={d.id} />{/each}
    </div>
    {#each smokes() as d (d.id)}<SmokeRow {d} name={nameOf(d.id)} />{/each}
    <Routines />
    {#if cameras().length}
      <h3>{t('commun.nav.cameras')}</h3>
      <div class="cards">
        {#each cameras() as d (d.id)}<section class="card"><CameraTile id={d.id} name={nameOf(d.id)} refresh={15} /></section>{/each}
      </div>
    {/if}
  {:else if panel === 'car'}
    <div class="cards">
      {#each chargers() as d (d.id)}<ChargerCard id={d.id} />{/each}
    </div>
  {:else if panel === 'energy'}
    <EnergyCard />
  {/if}

  {#if link}
    <a class="more" href="#/{link}">{t('maison.accueil.tout_voir', { page: t(`commun.nav.${link}`) })}<Icon name="chevron-right" size={17} /></a>
  {/if}
</aside>

<style>
  .apps {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .apps button {
    border: 0;
    border-radius: 999px;
    padding: 8px 14px;
    font: inherit;
    font-size: 14px;
    font-weight: 650;
    background: var(--surface-2);
    color: var(--ink-2);
    cursor: pointer;
  }

  .panel {
    position: fixed;
    z-index: 31;
    top: 12px;
    right: 12px;
    bottom: 12px;
    width: min(480px, calc(100vw - 24px));
    padding: 22px;
    overflow-y: auto;
    border-radius: 26px;
    outline: none;
    background: rgb(13 17 28 / 86%);
    backdrop-filter: blur(26px);
    color: var(--ink);
    box-shadow: inset 0 0 0 1px rgb(255 255 255 / 9%), 0 30px 80px -24px rgb(0 0 0 / 60%);
    animation: slide 0.28s var(--ease);
  }

  @keyframes slide {
    from {
      transform: translateX(28px);
      opacity: 0;
    }
  }

  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    margin-bottom: 14px;
  }

  h2 {
    margin: 0;
    font-family: var(--serif);
    font-size: 34px;
    font-weight: 380;
    letter-spacing: -0.015em;
    line-height: 1.1;
  }

  h3 {
    margin: 22px 0 6px;
    color: var(--ink-3);
    font-size: 13px;
    font-weight: 620;
  }

  .head-right {
    display: flex;
    align-items: center;
    gap: 14px;
  }

  .heat {
    font-size: 24px;
    white-space: nowrap;
  }

  .heat small {
    margin-left: 7px;
    color: var(--ink-3);
    font-size: 13px;
  }

  .muted {
    margin: 0 0 8px;
    color: var(--ink-3);
  }

  .wide {
    width: 100%;
    min-height: 48px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    border: 0;
    border-radius: 15px;
    background: var(--surface);
    color: inherit;
    font: inherit;
    font-weight: 620;
    cursor: pointer;
  }

  /* The panel is narrow: the alarm's state above its buttons. */
  .panel :global(.card.alarm) {
    grid-template-columns: minmax(0, 1fr);
  }

  .hush {
    margin-bottom: 8px;
  }

  .hush.on {
    background: color-mix(in srgb, var(--alert) 16%, var(--surface));
    color: var(--alert);
  }

  .modes {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 12px;
  }

  .modes button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 8px 14px;
    border: 0;
    border-radius: 999px;
    background: var(--surface);
    color: var(--ink-3);
    font: inherit;
    font-size: 14px;
    font-weight: 650;
    cursor: pointer;
  }

  .modes button.on {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .cards {
    display: grid;
    gap: 12px;
    margin-top: 12px;
  }

  .cards:empty {
    display: none;
  }

  .more {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 2px;
    margin-top: 18px;
    color: var(--ink-2);
    font-size: 14px;
    font-weight: 600;
    text-decoration: none;
  }

  .more:hover {
    color: var(--ink);
  }

  @media (max-width: 760px) {
    .panel {
      top: auto;
      right: 0;
      left: 0;
      bottom: 0;
      width: 100%;
      max-height: 88dvh;
      border-radius: 26px 26px 0 0;
      padding: 20px 16px calc(22px + env(safe-area-inset-bottom));
      animation-name: rise;
    }

    @keyframes rise {
      from {
        transform: translateY(28px);
        opacity: 0;
      }
    }
  }
</style>

<script>
  import { onMount } from 'svelte';
  import { home, hub, value, device, isOn, clock, num, relative, lastSeen, roomList, lampsOf, nameOf, doorOpen, leak, lowBattery, act, pending } from '../lib/home.svelte.js';
  import { isPrinter, phase } from '../lib/printers.js';
  import { t, locale } from '../../lib/i18n.svelte.js';
  import Icon from '../ui/Icon.svelte';
  import LightTile from '../ui/LightTile.svelte';
  import ClimateCard from '../ui/ClimateCard.svelte';
  import EnergyCard from '../ui/EnergyCard.svelte';
  import AlarmCard from '../ui/AlarmCard.svelte';
  import CameraTile from '../ui/CameraTile.svelte';
  import AmbiancePicker from '../ui/AmbiancePicker.svelte';
  import Routines from '../ui/Routines.svelte';
  import SkyHero from '../ui/SkyHero.svelte';
  import { setTile, saveDashboard, resetDashboard, onDashboard, isApart } from '../lib/layout.svelte.js';
  import TileBoard from '../ui/TileBoard.svelte';
  import { catalogue, showTile, isClim, isDev, available } from '../lib/tiles.svelte.js';
  import DeviceCard from '../ui/DeviceCard.svelte';
  import SensorCard from '../ui/SensorCard.svelte';
  import ThermoCard from '../ui/ThermoCard.svelte';
  import ChargerCard from '../ui/ChargerCard.svelte';
  import GarageCard from '../ui/GarageCard.svelte';
  import SmokeCard from '../ui/SmokeCard.svelte';
  import SpeakerCard from '../ui/SpeakerCard.svelte';
  import PlayerCard from '../ui/PlayerCard.svelte';
  import { has } from '../lib/speakers.js';
  import ShutterCard from '../ui/ShutterCard.svelte';
  import PrinterCard from '../ui/PrinterCard.svelte';
  import PaperPrinterCard from '../ui/PaperPrinterCard.svelte';
  import Remote from '../ui/Remote.svelte';
  import { deviceKind, isSmoke, isThermo, isCharger } from '../lib/home.svelte.js';
  import { isPaperPrinter } from '../lib/paper.js';

  const cfg = $derived(home.config ?? {});

  /** How a speaker's player shows on the dashboard: volume and progress, no
   *  grouping or library (those are the Speakers page's). */
  const PLAYER_LOOK = { sections: { volumes: true, join: false, library: false, progress: true }, ambient: true };

  /** Where the dashboard's arrangement is kept. */
  const DASH = { set: setTile, save: saveDashboard };

  const tiles = $derived(catalogue().filter((d) => isClim(d.id) || (isDev(d.id) ? !!device(d.device) : available(d.id) && (d.id !== 'lights' || shownLights.length > 0) && (d.id !== 'ambiances' || !!mood))));

  const out = $derived(cfg.outdoor ?? {});

  // The favourites if the house chose some, else every light of every room.
  // An alarm: a device that takes a `mode` among which « disarmed ».
  const alarm = $derived(
    Object.values(hub.devices).find((d) => d.points.some((p) => p.key === 'mode' && p.kind?.values?.includes('disarmed'))),
  );

  const lights = $derived.by(() => {
    const fav = cfg.favorites?.lights ?? [];
    if (fav.length) return fav;
    return roomList().flatMap((r) => r.lights.map((d) => ({ id: d.id, name: null, icon: 'light', also: [] })));
  });
  /** The lights the family wants on the dashboard (each has its own box in Équipements). */
  const shownLights = $derived(lights.filter((l) => onDashboard(l.id) && !isApart(l.id)));
  const litCount = $derived(shownLights.filter((l) => [l.id, ...(l.also ?? [])].some((i) => isOn(i))).length);

  // Ambiances, room by room (the last room chosen is remembered here).
  const moodRooms = $derived(roomList().map((r) => ({ name: r.name, lamps: lampsOf(r) })).filter((r) => r.lamps.length));
  let moodPick = $state((() => {
    try {
      return localStorage.getItem('moli.mood-room');
    } catch {
      return null;
    }
  })());
  const mood = $derived(moodRooms.find((r) => r.name === moodPick) ?? moodRooms.find((r) => r.name === 'Salon') ?? moodRooms[0]);
  function pickRoom(name) {
    moodPick = name;
    try {
      localStorage.setItem('moli.mood-room', name);
    } catch {
      /* remembered for this visit only */
    }
  }

  // The house's mode (a helper, « Mode Maison »): one tap, like Home
  // Assistant's mode buttons.
  const modeDevice = $derived(Object.values(hub.devices).find((d) => d.id.endsWith(':mode_maison')));
  const modes = $derived(modeDevice?.points.find((p) => p.key === 'state')?.kind.values ?? []);
  const mode = $derived(modeDevice ? value(modeDevice.id, 'state') : null);
  const MODE_ICON = { Normal: 'home', Nuit: 'moon', Absent: 'lock', Vacances: 'palm' };
  /** A mode's name in the house's language (an unknown one shows as it is). */
  function modeName(m) {
    const key = `maison.accueil.mode.${String(m).toLowerCase()}`;
    const word = t(key);
    return word === key ? m : word;
  }
  function setMode(m) {
    if (m !== mode) act(`${modeDevice.id}/state`, m, t('maison.accueil.mode_action', { mode: modeName(m) }));
  }

  // The speakers that talk (announcements): one tap silences them all, for
  // four hours at most (they speak again by themselves).
  const talkers = $derived(Object.values(hub.devices).filter((d) => d.points.some((p) => p.key === 'quiet' && p.access?.write)));
  const quiet = $derived(talkers.some((d) => value(d.id, 'quiet') === true));
  const quietUntil = $derived(Math.max(0, ...talkers.map((d) => Number(value(d.id, 'quiet_until') ?? 0))));
  function toggleQuiet() {
    for (const d of talkers) act(`${d.id}/quiet`, !quiet, quiet ? t('maison.accueil.annonces_retablies') : t('maison.accueil.annonces_coupees'));
  }

  // What deserves a word right now. Silence when all is well.
  const notices = $derived.by(() => {
    const list = [];
    // Smoke and water from any sensor that knows it, not only the listed ones.
    const all = Object.values(hub.devices);
    for (const d of all) {
      if (value(d.id, 'smoke') === true) list.push({ tone: 'alert', icon: 'fire', text: t('maison.accueil.fumee', { name: nameOf(d.id) }), href: '#/pieces' });
      if (leak(d.id)) list.push({ tone: 'alert', icon: 'water', text: t('maison.accueil.fuite', { name: nameOf(d.id) }), href: '#/pieces' });
    }
    if (hub.approvals?.length) list.push({ tone: 'cool', icon: 'lock', text: t('maison.accueil.demandes', { count: hub.approvals.length }), href: '#/reglages/securite' });
    for (const door of cfg.doors ?? []) {
      const d = device(door.id);
      if (!d) continue;
      if (doorOpen(door.id) === true) list.push({ tone: 'warm', icon: 'door-open', text: t('maison.accueil.porte_ouverte', { name: door.name }), href: '#/pieces' });
    }
    const bell = (cfg.favorites?.cameras ?? [])[0]?.id;
    if (bell && value(bell, 'doorbell') === true) list.push({ tone: 'alert', icon: 'bell', text: t('maison.accueil.sonnette'), href: '#/cameras' });
    if (bell && value(bell, 'person') === true) list.push({ tone: 'warm', icon: 'account', text: t('maison.accueil.quelquun'), href: '#/cameras' });
    for (const p of Object.values(hub.devices).filter(isPrinter)) {
      if (p.online === false) continue;
      const st = phase(value(p.id, 'state'));
      const left = value(p.id, 'remaining');
      const pct = Math.round(Number(value(p.id, 'progress') ?? 0));
      if (st === 'printing') {
        const text =
          typeof left === 'number'
            ? t('maison.accueil.impression_fin', { name: nameOf(p.id), pct, time: clock(home.now + left * 60_000) })
            : t('maison.accueil.impression', { name: nameOf(p.id), pct });
        list.push({ tone: 'cool', icon: 'printer3d', text, href: '#/impression' });
      } else if (st === 'paused') {
        list.push({ tone: 'warm', icon: 'printer3d', text: t('maison.accueil.impression_pause', { name: nameOf(p.id), pct }), href: '#/impression' });
      } else if (st === 'error') {
        list.push({ tone: 'alert', icon: 'printer3d', text: t('maison.accueil.impression_erreur', { name: nameOf(p.id) }), href: '#/impression' });
      }
    }
    if (value('energie:maison', 'over_budget') === true) {
      const bill = value('energie:maison', 'bill_projection');
      list.push({ tone: 'warm', icon: 'bolt', text: t('maison.accueil.facture', { bill: num(bill) }), href: '#/energie' });
    }
    // A dead battery takes its sensor offline: count it all the same.
    const flat = all.filter((d) => lowBattery(d.id));
    if (flat.length) {
      const text =
        flat.length <= 3
          ? t('maison.accueil.piles_liste', { count: flat.length, names: flat.map((d) => nameOf(d.id)).join(', ') })
          : t('maison.accueil.piles', { count: flat.length });
      list.push({ tone: 'cool', icon: 'alert', text, href: '#/pieces' });
    }
    if (out.pool && value(out.pool, 'action_required') === true) list.push({ tone: 'cool', icon: 'pool', text: t('maison.accueil.piscine_oeil'), href: '#/dehors' });
    return list;
  });

  const tv = $derived(cfg.salon?.tv);
  const speaker = $derived(cfg.salon?.speaker);
  const tvOn = $derived(value(tv, 'power') === true);
  const playing = $derived(value(speaker, 'playing') === true);
  const aqi = $derived(value(out.air, 'aqi'));
  const aqiWord = $derived(
    aqi == null
      ? '—'
      : aqi <= 20
        ? t('maison.air.tres_bon')
        : aqi <= 40
          ? t('maison.air.bon')
          : aqi <= 60
            ? t('maison.air.moyen')
            : aqi <= 80
              ? t('maison.air.mediocre')
              : t('maison.air.mauvais'),
  );
  const bell = $derived((cfg.favorites?.cameras ?? [])[0]);

  // The last real passage at the door (the history knows; the live value
  // is refreshed every few seconds whatever happens).
  let passage = $state(null);
  onMount(() => {
    const refresh = () => bell && lastSeen(bell.id).then((ts) => (passage = ts));
    const t = setInterval(refresh, 120_000);
    return () => clearInterval(t);
  });
  $effect(() => {
    if (bell) lastSeen(bell.id).then((ts) => (passage = ts));
  });
</script>

<div class="accueil">
  {#snippet body(tile)}
    {#if isClim(tile.id)}
      <ClimateCard id={tile.device} name={tile.label} />
    {:else if isDev(tile.id)}
      {@const d = device(tile.device)}
      {@const kind = deviceKind(d)}
      {#if kind === 'camera'}
        <section class="card doorbell">
          <div class="card-head"><h2><Icon name="cctv" size={18} />{tile.label}</h2></div>
          <CameraTile id={tile.device} name={tile.label} refresh={15} />
        </section>
      {:else if kind === 'garage'}
        <GarageCard id={tile.device} name={tile.label} />
      {:else if kind === 'light'}
        <LightTile id={tile.device} name={tile.label} dimmer />
      {:else if kind === 'plug'}
        <LightTile id={tile.device} name={tile.label} icon="power" />
      {:else if kind === 'cover'}
        <ShutterCard id={tile.device} name={tile.label} />
      {:else if kind === 'speaker'}
        {#if has(tile.device, 'group_leader')}
          <!-- A Sonos room: the same player as on the Speakers page. -->
          <PlayerCard members={[d]} rooms={[]} look={PLAYER_LOOK} groupKey={tile.id} />
        {:else}
          <SpeakerCard id={tile.device} name={tile.label} />
        {/if}
      {:else if kind === 'tv'}
        <Remote tv={tile.device} speaker={speaker} />
      {:else if kind === 'printer'}
        {#if isPaperPrinter(d)}<PaperPrinterCard id={tile.device} />{:else}<PrinterCard id={tile.device} />{/if}
      {:else if isSmoke(d)}
        <SmokeCard id={tile.device} name={tile.label} />
      {:else if isCharger(d)}
        <ChargerCard id={tile.device} name={tile.label} />
      {:else if isThermo(d)}
        <ThermoCard id={tile.device} name={tile.label} />
      {:else}
        <DeviceCard id={tile.device} name={tile.label} />
      {/if}
    {:else if tile.id === 'sky'}
      <SkyHero />
    {:else if tile.id === 'modes'}
    <div class="modes" role="radiogroup" aria-label={t('maison.accueil.mode_maison')}>
      {#each modes as m (m)}
        <button role="radio" aria-checked={m === mode} class:on={m === mode} disabled={pending(modeDevice.id, 'state')} onclick={() => setMode(m)}>
          <Icon name={MODE_ICON[m] ?? 'home'} size={16} />{modeName(m)}
        </button>
      {/each}
      {#if talkers.length}
        <span class="sep" aria-hidden="true"></span>
        <button class="hush" class:on={quiet} aria-pressed={quiet} onclick={toggleQuiet} title={quiet ? t('maison.accueil.sonos_reparle') : t('maison.accueil.sonos_coupe')}>
          <Icon name={quiet ? 'volume-off' : 'speaker'} size={16} />{quiet && quietUntil ? t('maison.accueil.sonos_muette_jusqua', { time: new Date(quietUntil).toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit' }) }) : t('maison.accueil.sonos_muette')}
        </button>
      {/if}
    </div>
  
    {:else if tile.id === 'notices'}
  <section class="notices" aria-label={t('maison.accueil.a_savoir')}>
    {#each notices as n (n.text)}
      <a class="chip {n.tone}" href={n.href}><Icon name={n.icon} size={16} />{n.text}</a>
    {:else}
      <span class="chip good"><Icon name="leaf" size={16} />{t('maison.accueil.calme')}</span>
    {/each}
  </section>


    {:else if tile.id === 'alarm'}
      <AlarmCard id={alarm.id} />
    {:else if tile.id === 'lights'}
    <section class="card lights" >
      <div class="card-head">
        <h2><Icon name="light" size={18} />{t('maison.accueil.lumieres')} <span class="count">{litCount ? t('maison.accueil.allumees', { count: litCount }) : t('maison.accueil.tout_eteint')}</span></h2>
        <a class="more" href="#/pieces" aria-label={t('maison.accueil.toutes_pieces')}><Icon name="arrow-top-right" size={18} /></a>
      </div>
      <div class="tiles">
        {#each shownLights as l (l.id)}
          <LightTile id={l.id} name={l.name} icon={l.icon} also={l.also ?? []} dimmer />
        {/each}
      </div>
    </section>

    {:else if tile.id === 'ambiances'}
    <section class="card lights ambiances">
      <div class="moods">
        <div class="moods-head">
          <span class="moods-title"><Icon name="palette" size={16} />{t('maison.accueil.ambiance')}</span>
          <div class="rooms" role="group" aria-label={t('maison.accueil.piece')}>
            {#each moodRooms as r (r.name)}
              <button class:on={r.name === mood.name} onclick={() => pickRoom(r.name)} aria-pressed={r.name === mood.name}>{r.name}</button>
            {/each}
          </div>
        </div>
        <AmbiancePicker lights={mood.lamps} label={mood.name} />
      </div>
    </section>


    {:else if tile.id === 'energy'}
      <EnergyCard />
    {:else if tile.id === 'routines'}
      <Routines />
    {:else if tile.id === 'doorbell'}
      <section class="card doorbell">
        <div class="card-head">
          <h2><Icon name="bell" size={18} />{t('maison.accueil.devant_porte')}</h2>
          <a class="more" href="#/cameras" aria-label={t('maison.accueil.toutes_cameras')}><Icon name="arrow-top-right" size={18} /></a>
        </div>
        <CameraTile id={bell.id} name={bell.name} refresh={15} />
        <p class="muted small">{passage ? t('maison.accueil.dernier_passage', { when: relative(passage, home.now) }) : t('maison.accueil.personne')}</p>
      </section>
    
    {:else if tile.id === 'remote'}
    <a class="card remote" href="#/salon">
      <span class="tv-object" aria-hidden="true">
        <span class="screen" class:lit={tvOn}></span>
        <span class="stand"></span>
      </span>
      <span class="remote-text">
        <strong>{t('maison.accueil.telecommande')}</strong>
        <span class="muted">{tvOn ? t('maison.accueil.tele_allumee') : t('maison.accueil.tele_veille')}{playing ? ` · ${t('maison.accueil.son_joue')}` : ''}</span>
      </span>
      <span class="go"><Icon name="arrow-top-right" size={20} /></span>
    </a>
    
    {:else if tile.id === 'outside'}
    <a class="card outside" href="#/dehors">
      <div class="card-head"><h2><Icon name="flower-outline" size={18} />{t('maison.accueil.dehors')}</h2><span class="more" aria-hidden="true"><Icon name="arrow-top-right" size={18} /></span></div>
      <div class="facts">
        <div><small>{t('maison.accueil.air')}</small><b>{aqiWord}</b></div>
        <div><small>UV</small><b class="num">{num(value(out.air, 'uv_index'), 0)}</b></div>
        <div><small>{t('maison.accueil.humidite')}</small><b class="num">{value(out.weather, 'humidity') != null ? `${num(value(out.weather, 'humidity'))} %` : '—'}</b></div>
        <div><small>{t('maison.accueil.piscine')}</small><b class="num">{value(out.pool, 'temperature') != null ? `${num(value(out.pool, 'temperature'), 1)}°` : '—'}</b></div>
      </div>
    </a>
    
    {/if}
  {/snippet}

  {#snippet hint()}{t('maison.dashboard.aide')} <a href="#/equipements">{t('maison.dashboard.masquees')}</a>{/snippet}

  <TileBoard title={t('maison.dashboard.titre')} {tiles} board={DASH} {body} {hint} onhide={(tile) => showTile(tile, false)} onreset={resetDashboard} />
</div>

<style>
  .accueil {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 22px;
  }

  .modes {
    display: flex;
    gap: 8px;
    overflow-x: auto;
    scrollbar-width: none;
    min-width: 0;
  }

  .modes button {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 0;
    border-radius: 999px;
    padding: 8px 14px;
    font: inherit;
    font-size: 14px;
    font-weight: 650;
    background: var(--surface-2);
    color: var(--ink-3);
    cursor: pointer;
  }

  .modes button.on {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .modes .sep {
    flex: none;
    width: 1px;
    margin: 6px 4px;
    background: var(--line, color-mix(in srgb, var(--ink) 14%, transparent));
  }

  .modes button.hush.on {
    background: color-mix(in srgb, var(--alert) 16%, var(--surface-2));
    color: var(--alert);
  }

  .notices {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
  }

  .notices a {
    text-decoration: none;
  }

  .notices .chip {
    padding: 9px 14px;
    font-size: 14px;
  }


  .ambiances .moods {
    margin: 0;
    padding: 0;
    border: 0;
  }

  .moods {
    grid-template-columns: minmax(0, 1fr);
    margin-top: 18px;
    padding-top: 16px;
    border-top: 1px solid var(--line);
    display: grid;
    gap: 12px;
  }

  .moods-head {
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 10px 14px;
    flex-wrap: wrap;
  }

  .moods-title {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-weight: 700;
    font-size: 14px;
  }

  .rooms {
    display: flex;
    gap: 6px;
    overflow-x: auto;
    scrollbar-width: none;
    min-width: 0;
    flex: 1;
  }

  .rooms button {
    flex: none;
    border: 0;
    border-radius: 999px;
    padding: 6px 12px;
    font: inherit;
    font-size: 13px;
    font-weight: 650;
    background: var(--surface-2);
    color: var(--ink-3);
    cursor: pointer;
  }

  .rooms button.on {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .count {
    font-weight: 550;
    color: var(--ink-3);
    margin-left: 4px;
  }

  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(172px, 1fr));
    gap: 12px;
  }

  .small {
    font-size: 13px;
    margin-top: 10px;
  }

  .remote {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 14px;
    color: inherit;
    text-decoration: none;
    min-height: 168px;
    align-content: space-between;
    transition: transform 0.2s var(--ease);
  }

  .remote:hover,
  .outside:hover {
    transform: translateY(-2px);
  }

  .tv-object {
    grid-column: 1 / -1;
    display: grid;
    justify-items: center;
  }

  .screen {
    width: 96px;
    height: 56px;
    border-radius: 8px;
    background: #171c28;
    box-shadow: inset 0 0 0 3px #2a3142;
    transition: background 0.4s;
  }

  .screen.lit {
    background: linear-gradient(135deg, #5fb4f0, #8f8cf5 55%, #f0a53a);
    box-shadow: inset 0 0 0 3px #2a3142, 0 8px 26px -6px #8f8cf5aa;
  }

  .stand {
    width: 30px;
    height: 6px;
    border-radius: 0 0 6px 6px;
    background: #2a3142;
  }

  .remote-text {
    display: grid;
  }

  .remote-text strong {
    font-size: 18px;
  }

  .remote-text span {
    font-size: 13px;
  }

  .go {
    align-self: end;
    width: 36px;
    height: 36px;
    border-radius: 50%;
    background: var(--surface-2);
    display: grid;
    place-items: center;
    color: var(--ink-2);
  }

  .outside {
    color: inherit;
    text-decoration: none;
    transition: transform 0.2s var(--ease);
  }

  .facts {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(72px, 1fr));
    gap: 10px 8px;
  }

  .facts small {
    display: block;
    font-size: 11px;
    color: var(--ink-3);
    font-weight: 650;
  }

  .facts b {
    font-size: 17px;
    font-weight: 650;
  }

  @media (max-width: 760px) {
    .tiles {
      grid-template-columns: 1fr 1fr;
    }
  }
</style>

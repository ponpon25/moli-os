<script>
  /** What a room holds, ready to be set: tall sliders for its lights and
   *  shutters, a line for what only switches, and the real cards (their guard,
   *  their code) for what needs more. `sensors`: its readings too. */
  import { act, deviceKind, isCharger, isSmoke, lampsOf, nameOf, reachable, setPower, value } from '../lib/home.svelte.js';
  import { coverOf, isAlarm, lightOf, setLight, shortName } from '../lib/accueil.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';
  import Fader from './Fader.svelte';
  import CoverRow from './CoverRow.svelte';
  import SmokeRow from './SmokeRow.svelte';
  import SpeakerRow from './SpeakerRow.svelte';
  import SwitchRow from './SwitchRow.svelte';
  import AlarmCard from './AlarmCard.svelte';
  import AmbiancePicker from './AmbiancePicker.svelte';
  import CameraTile from './CameraTile.svelte';
  import ChargerCard from './ChargerCard.svelte';
  import ClimateCard from './ClimateCard.svelte';
  import GarageCard from './GarageCard.svelte';
  import SensorCard from './SensorCard.svelte';
  import ThermoCard from './ThermoCard.svelte';

  let { room, sensors = false } = $props();

  const kinds = (...list) => room.devices.filter((d) => list.includes(deviceKind(d)) && !isCharger(d));
  const lamps = $derived(kinds('light'));
  const shutters = $derived(kinds('cover'));
  const moods = $derived(lampsOf(room.room));
  const speakers = $derived(kinds('speaker'));
  // A thermometer, not the weather forecast (which reads a temperature too).
  const isThermo = (d) => d.points.some((p) => p.key === 'temperature') && !d.points.some((p) => p.key === 'weather_code');
  /** Thermometers are always shown (their curve); other sensors only where asked. */
  const readings = $derived(kinds('sensor').filter((d) => !isAlarm(d) && !isSmoke(d) && !isCharger(d) && (sensors || isThermo(d))));
  const short = (d) => shortName(d, room.name);
</script>

{#each room.devices.filter(isAlarm) as d (d.id)}<AlarmCard id={d.id} />{/each}

{#if lamps.some((d) => lightOf(d).key) || shutters.some((d) => coverOf(d).settable)}
  <div class="faders">
    {#each lamps.filter((d) => lightOf(d).key) as d (d.id)}
      {@const l = lightOf(d)}
      <Fader kind="light" label={short(d)} value={l.pct} word={l.word} disabled={l.cut || !l.ok} onchange={(p) => setLight(d, p)} ontap={() => setPower(d, !l.on, nameOf(d.id))} />
    {/each}
    {#each shutters.filter((d) => coverOf(d).settable) as d (d.id)}
      {@const c = coverOf(d)}
      <Fader kind="cover" label={short(d)} value={c.pct ?? 0} word={c.word} disabled={!reachable(d.id)} onchange={(p) => act(`${d.id}/percent_control`, p, nameOf(d.id))} />
    {/each}
  </div>
{/if}

{#each lamps.filter((d) => !lightOf(d).key) as d (d.id)}<SwitchRow {d} icon="light" name={short(d)} />{/each}
{#each shutters.filter((d) => !coverOf(d).settable) as d (d.id)}<CoverRow {d} name={short(d)} />{/each}
{#each kinds('plug') as d (d.id)}<SwitchRow {d} name={short(d)} />{/each}
{#each kinds('tv') as d (d.id)}
  <a class="h-line" href="#/salon">
    <div class="h-row">
      <span class="h-ico" class:lit={value(d.id, 'power') === true}><Icon name="tv" size={19} /></span>
      <b>{short(d)}</b>
      <small>{value(d.id, 'power') === true ? t('maison.accueil.allumee') : t('maison.accueil.veille')}</small>
      <Icon name="chevron-right" size={18} />
    </div>
  </a>
{/each}
{#each room.devices.filter(isSmoke) as d (d.id)}<SmokeRow {d} name={short(d)} />{/each}

{#if moods.length}
  <h3>{t('maison.accueil.ambiance')}</h3>
  <AmbiancePicker lights={moods} label={room.name} />
{/if}

{#if speakers.length}<h3>{t('maison.accueil.usage.music')}</h3>{/if}
{#each speakers as d (d.id)}<SpeakerRow {d} />{/each}

<div class="cards">
  {#each kinds('climate') as d (d.id)}<ClimateCard id={d.id} name={short(d)} />{/each}
  {#each kinds('garage') as d (d.id)}<GarageCard id={d.id} />{/each}
  {#each room.devices.filter(isCharger) as d (d.id)}<ChargerCard id={d.id} />{/each}
  {#each kinds('camera') as d (d.id)}<section class="card"><CameraTile id={d.id} name={nameOf(d.id)} refresh={15} /></section>{/each}
  <!-- A thermometer gets its curve, as on the home page; other sensors their readings in words. -->
  {#each readings as d (d.id)}{#if isThermo(d)}<ThermoCard id={d.id} name={short(d)} />{:else}<SensorCard id={d.id} />{/if}{/each}
</div>

<style>
  h3 {
    margin: 22px 0 6px;
    color: var(--ink-3);
    font-size: 13px;
    font-weight: 620;
  }

  .faders {
    display: flex;
    flex-wrap: wrap;
    gap: 14px 12px;
    margin: 12px 0;
  }

  .cards {
    display: grid;
    gap: 12px;
    margin-top: 12px;
  }

  .cards:empty {
    display: none;
  }

  /* The alarm's state above its buttons, wherever the room is shown. */
  :global(.card.alarm) {
    grid-template-columns: minmax(0, 1fr);
  }
</style>

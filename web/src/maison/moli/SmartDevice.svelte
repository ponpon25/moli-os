<script>
  import { home, device, deviceKind, nameOf, isSmoke, isThermo, isCharger } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import LightTile from '../ui/LightTile.svelte';
  import ClimateCard from '../ui/ClimateCard.svelte';
  import CameraTile from '../ui/CameraTile.svelte';
  import CoverTile from '../ui/CoverTile.svelte';
  import SpeakerCard from '../ui/SpeakerCard.svelte';
  import SensorCard from '../ui/SensorCard.svelte';
  import SmokeCard from '../ui/SmokeCard.svelte';
  import ThermoCard from '../ui/ThermoCard.svelte';
  import ChargerCard from '../ui/ChargerCard.svelte';
  import GarageCard from '../ui/GarageCard.svelte';
  import WeatherCard from './WeatherCard.svelte';

  /** Any device, shown the way it is used. */
  let { id, title = null } = $props();

  const d = $derived(device(id));
  const kind = $derived(deviceKind(d));
  const cam = $derived((home.config?.favorites?.cameras ?? []).find((c) => c.id === id));
</script>

{#if !d}
  <section class="card"><p class="muted">{t('moli.appareil.introuvable')}</p></section>
{:else if id === home.config?.outdoor?.weather}
  <WeatherCard />
{:else if kind === 'camera'}
  <CameraTile {id} name={title ?? cam?.name ?? nameOf(id)} refresh={cam?.refresh ?? 10} battery={cam?.battery} onopen={() => (location.hash = '#/cameras')} />
{:else if kind === 'garage'}
  <GarageCard {id} name={title} />
{:else if kind === 'climate'}
  <ClimateCard {id} name={title ?? nameOf(id)} />
{:else if kind === 'speaker'}
  <SpeakerCard {id} name={title} />
{:else if kind === 'tv'}
  <div class="card tight"><LightTile {id} point="power" name={title} icon="tv" /></div>
{:else if kind === 'cover'}
  <div class="card tight"><CoverTile {id} name={title} /></div>
{:else if kind === 'light' || kind === 'group'}
  <div class="card tight"><LightTile {id} name={title} dimmer /></div>
{:else if kind === 'plug'}
  <div class="card tight"><LightTile {id} name={title} icon="flash" /></div>
{:else if isSmoke(d)}
  <SmokeCard {id} name={title} />
{:else if isCharger(d)}
  <ChargerCard {id} name={title} />
{:else if isThermo(d)}
  <ThermoCard {id} name={title} />
{:else}
  <SensorCard {id} name={title} />
{/if}

<style>
  .tight {
    padding: 8px;
  }
</style>

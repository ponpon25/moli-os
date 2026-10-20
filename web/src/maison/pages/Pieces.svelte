<script module>
  /** The plan, kept between visits: the floors show at once the next time. */
  let cachedPlan = null;
</script>

<script>
  import { onMount } from 'svelte';
  import { hub, roomList, roomNamed, roomOf, savePref, protectedRoom } from '../lib/home.svelte.js';
  import { layout, hideRoom, addRoom, removeRoom } from '../lib/layout.svelte.js';
  import Icon from '../ui/Icon.svelte';
  import RoomCard from '../ui/RoomCard.svelte';
  import FilterChips from '../ui/FilterChips.svelte';
  import { categoryOf, loadFilter } from '../lib/filters.js';
  import { floorsOf } from '../lib/floors.js';
  import { t } from '../../lib/i18n.svelte.js';

  let plan = $state(cachedPlan);
  onMount(async () => {
    try {
      const res = await fetch('/api/plan');
      if (res.ok) plan = cachedPlan = await res.json();
    } catch {
      /* without the plan: one list, no floors */
    }
  });

  /** Which kinds of devices show (none chosen: all). */
  let filter = $state(loadFilter('pieces-filter'));
  /** Pads with their names or icons only: this device's choice. */
  let look = $state((() => {
    try {
      return localStorage.getItem('pieces-look') === 'icons' ? 'icons' : 'text';
    } catch {
      return 'text';
    }
  })());
  function choose(v) {
    look = v;
    savePref('pieces-look', v);
  }

  /** Arranging: hidden rooms show (dimmed), rooms can be added and removed. */
  let editing = $state(false);
  let draft = $state('');
  let asking = $state(null);

  const empty = (name) => ({ name, icon: 'home', group: null, lights: [], plugs: [], covers: [], climates: [], media: [], sensors: [], lit: 0 });
  const full = $derived.by(() => {
    const list = roomList();
    return [...list, ...layout.extraRooms.filter((n) => !list.some((r) => r.name === n)).map(empty)];
  });
  const all = $derived(full.filter((r) => editing || !layout.hiddenRooms.includes(r.name)));
  const count = (r) => r.lights.length + r.plugs.length + r.covers.length + r.climates.length + r.media.length + r.sensors.length + (r.group ? 1 : 0);
  const labelled = (name) => Object.values(hub.devices).filter((d) => roomOf(d) === name && d.label?.room);

  function add(e) {
    e.preventDefault();
    if (addRoom(draft)) draft = '';
  }
  async function remove(name) {
    asking = null;
    await removeRoom(name, labelled(name));
  }
  const devices = $derived(
    all.flatMap((r) => [r.group, ...r.lights, ...r.plugs, ...r.covers, ...r.climates, ...r.media, ...r.sensors].filter(Boolean)),
  );

  /** A room with only what the filter keeps; null when nothing is left. */
  function only(r) {
    if (!filter.length) return r;
    const on = (id) => filter.includes(id);
    const f = {
      ...r,
      group: on('lumieres') ? r.group : null,
      lights: on('lumieres') ? r.lights : [],
      plugs: on('prises') ? r.plugs : [],
      covers: on('climat') ? r.covers : [],
      climates: on('climat') ? r.climates : [],
      media: on('medias') ? r.media : [],
      sensors: r.sensors.filter((d) => on(categoryOf(d))),
    };
    if (!count(r)) return f;
    const any = f.group || f.lights.length || f.plugs.length || f.covers.length || f.climates.length || f.media.length || f.sensors.length;
    return any ? f : null;
  }

  const rooms = $derived(all.map(only).filter(Boolean));

  // Floor by floor (from all the rooms: a filter does not move one).
  const placed = $derived(floorsOf(all, plan, roomNamed));
  const floors = $derived(
    placed.sections
      .map((s) => ({ ...s, rooms: rooms.filter((r) => placed.of.get(r.name) === s.id) }))
      .filter((s) => s.rooms.length),
  );
  const litIn = (list) => list.reduce((n, r) => n + r.lit, 0);
</script>

<div class="pieces">
  <header>
    <h1 class="page-title">{t('maison.pieces.titre')}</h1>
    <p class="page-sub">{t('maison.pieces.sous_titre')}</p>
    <button class="edit" class:active={editing} onclick={() => (editing = !editing)}>
      <Icon name={editing ? 'check' : 'pencil'} size={16} />{editing ? t('maison.pieces.terminer') : t('maison.pieces.gerer')}
    </button>
  </header>

  {#if editing}
    <form class="add" onsubmit={add}>
      <input bind:value={draft} maxlength="40" placeholder={t('maison.pieces.nom_nouvelle')} aria-label={t('maison.pieces.nom_nouvelle')} />
      <button type="submit" disabled={!draft.trim()}><Icon name="plus" size={16} />{t('maison.pieces.ajouter')}</button>
      <a href="#/equipements">{t('maison.pieces.rattacher')}</a>
    </form>
  {/if}

  <div class="bar">
    <FilterChips {devices} bind:filter key="pieces-filter" />
    <div class="look" role="radiogroup" aria-label={t('maison.pieces.affichage')}>
      <button role="radio" aria-checked={look === 'text'} class:active={look === 'text'} onclick={() => choose('text')} title={t('maison.pieces.avec_noms')}>Aa</button>
      <button role="radio" aria-checked={look === 'icons'} class:active={look === 'icons'} onclick={() => choose('icons')} title={t('maison.pieces.icones')}><Icon name="light" size={15} /></button>
    </div>
  </div>

  {#each floors as floor (floor.id)}
    <section class="floor" aria-label={floor.name ?? t('maison.pieces.titre')}>
      {#if floors.length > 1 && floor.name}
        {@const lit = litIn(floor.rooms)}
        <h2 class="floor-title">
          <span>{floor.name}</span>
          {#if lit}<small>{t('maison.pieces.allumees', { count: lit })}</small>{/if}
        </h2>
      {/if}
      <div class="rooms">
        {#each floor.rooms as room (room.name)}
          {@const off = layout.hiddenRooms.includes(room.name)}
          <div class="slot" class:off>
            {#if editing}
              <div class="manage">
                <span class="sp"></span>
                {#if protectedRoom(room.name)}<Icon name="lock" size={14} />{/if}
                <button onclick={() => hideRoom(room.name, !off)} title={off ? t('maison.pieces.afficher') : t('maison.pieces.masquer')} aria-label={off ? t('maison.pieces.afficher') : t('maison.pieces.masquer')}>
                  <Icon name={off ? 'eye' : 'eye-off'} size={16} />
                </button>
                {#if asking === room.name}
                  <button class="danger" onclick={() => remove(room.name)}>{t('maison.pieces.confirmer')}</button>
                  <button onclick={() => (asking = null)}>{t('commun.annuler')}</button>
                {:else}
                  <button onclick={() => (asking = room.name)} title={t('maison.pieces.supprimer')} aria-label={t('maison.pieces.supprimer')}><Icon name="trash" size={16} /></button>
                {/if}
              </div>
            {/if}
            {#if count(room)}
              <RoomCard {room} every={filter.includes('capteurs')} icons={look === 'icons'} />
            {:else}
              <p class="vide"><b>{room.name}</b><br />{t('maison.pieces.vide')} <a href="#/equipements">{t('maison.pieces.rattacher')}</a></p>
            {/if}
          </div>
        {/each}
      </div>
    </section>
  {/each}
</div>

<style>
  .pieces {
    display: grid;
    gap: 22px;
  }

  .bar {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }

  .bar > :global(:first-child) {
    flex: 1;
    min-width: 0;
  }

  .look {
    display: inline-flex;
    padding: 3px;
    border-radius: 999px;
    background: var(--surface-2);
  }

  .look button {
    display: grid;
    place-items: center;
    min-width: 34px;
    height: 28px;
    border: none;
    border-radius: 999px;
    background: transparent;
    color: var(--ink-3);
    font: inherit;
    font-size: 13px;
    font-weight: 700;
    cursor: pointer;
  }

  .look button.active {
    background: var(--surface);
    color: var(--ink);
    box-shadow: 0 1px 3px rgb(0 0 0 / 15%);
  }

  header {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 16px;
  }

  header .page-sub {
    flex: 1;
    min-width: 200px;
  }

  .edit,
  .add button,
  .manage button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 0;
    border-radius: 999px;
    padding: 7px 14px;
    font: inherit;
    font-size: 13px;
    font-weight: 650;
    background: var(--surface-2);
    color: var(--ink-2);
    cursor: pointer;
  }

  .edit.active {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .add {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px;
  }

  .add input {
    flex: 1;
    min-width: 180px;
    max-width: 320px;
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 8px 14px;
    font: inherit;
    background: var(--surface);
    color: var(--ink);
  }

  .add button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .add a,
  .vide a {
    font-size: 13px;
    font-weight: 650;
    color: var(--warm-ink);
  }

  .manage {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 8px;
  }

  .manage .sp {
    flex: 1;
  }

  .manage button {
    padding: 6px 10px;
  }

  .manage .danger {
    background: color-mix(in srgb, var(--alert) 16%, var(--surface-2));
    color: var(--alert);
  }

  .slot.off > :global(:not(.manage)) {
    opacity: 0.45;
  }

  .vide {
    margin: 0;
    padding: 18px;
    border-radius: 16px;
    border: 1px dashed var(--line);
    color: var(--ink-3);
    font-size: 13px;
  }

  .floor {
    display: grid;
    gap: 12px;
  }

  .floor-title {
    display: flex;
    align-items: baseline;
    gap: 10px;
    margin: 0;
    font-size: 13px;
    font-weight: 750;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ink-2);
  }

  /* A thin rule after the name, to the edge. */
  .floor-title::after {
    content: '';
    flex: 1;
    align-self: center;
    height: 1px;
    background: color-mix(in srgb, var(--ink-3) 25%, transparent);
  }

  .floor-title small {
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0;
    text-transform: none;
    color: var(--warm-ink);
  }

  .rooms {
    columns: 3 340px;
    column-gap: 20px;
  }

  .slot {
    break-inside: avoid;
    margin-bottom: 20px;
  }
</style>

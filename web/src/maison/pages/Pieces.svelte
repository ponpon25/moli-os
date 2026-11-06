<script module>
  /** The plan, kept between visits: the floors show at once the next time. */
  let cachedPlan = null;
</script>

<script>
  // The rooms: the list on one side, the chosen room ready to be set on the
  // other (on a phone, it opens over the list). « Gérer » hides, adds and
  // removes rooms.
  import { onMount } from 'svelte';
  import { hub, num, roomList, roomNamed, roomOf, protectedRoom } from '../lib/home.svelte.js';
  import { rooms as liveRooms } from '../lib/accueil.svelte.js';
  import { layout, hideRoom, addRoom, removeRoom } from '../lib/layout.svelte.js';
  import { floorsOf } from '../lib/floors.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from '../ui/Icon.svelte';
  import RoomTile from '../ui/RoomTile.svelte';
  import RoomDetail from '../ui/RoomDetail.svelte';
  import Sortable from '../ui/Sortable.svelte';

  let { route = 'pieces' } = $props();

  let plan = $state(cachedPlan);
  let wide = $state(true);
  onMount(() => {
    const media = matchMedia('(min-width: 900px)');
    wide = media.matches;
    const update = (e) => (wide = e.matches);
    media.addEventListener('change', update);
    (async () => {
      try {
        const res = await fetch('/api/plan');
        if (res.ok) plan = cachedPlan = await res.json();
      } catch {
        /* without the plan: one list, no floors */
      }
    })();
    return () => media.removeEventListener('change', update);
  });

  /** Arranging: hidden rooms show (dimmed), rooms can be added and removed. */
  let editing = $state(false);
  let draft = $state('');
  let asking = $state(null);

  const known = $derived(liveRooms(true));
  const empty = (name) => ({ name, gear: [], devices: [], room: null, temp: null, humidity: null, lit: false });
  const full = $derived.by(() => {
    const names = [...new Set([...known.map((r) => r.name), ...layout.extraRooms])];
    return names.map((n) => known.find((r) => r.name === n) ?? empty(n));
  });
  const shown = $derived(full.filter((r) => editing || !layout.hiddenRooms.includes(r.name)));

  // Floor by floor, as the plan stacks them.
  const placed = $derived(floorsOf(roomList(), plan, roomNamed));
  const floors = $derived(
    placed.sections
      .map((s) => ({ ...s, rooms: shown.filter((r) => placed.of.get(r.name) === s.id) }))
      .filter((s) => s.rooms.length),
  );
  const loose = $derived(shown.filter((r) => !placed.of.has(r.name)));

  const asked = $derived(decodeURIComponent(route.split('/')[1] ?? ''));
  const chosen = $derived(full.find((r) => r.name === asked) ?? (wide ? (shown[0] ?? null) : null));
  const go = (name) => (location.hash = `#/pieces/${encodeURIComponent(name)}`);
  const closeSheet = () => (location.hash = '#/pieces');

  const labelled = (name) => Object.values(hub.devices).filter((d) => roomOf(d) === name && d.label?.room);
  function add(e) {
    e.preventDefault();
    if (addRoom(draft)) draft = '';
  }
  async function remove(name) {
    asking = null;
    await removeRoom(name, labelled(name));
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && !wide && asked && closeSheet()} />

{#snippet tile(r)}
  <RoomTile {r} active={wide && chosen?.name === r.name} onclick={() => go(r.name)} />
{/snippet}

{#snippet manage(r)}
  {#if editing}
    {@const off = layout.hiddenRooms.includes(r.name)}
    <div class="manage">
      {#if protectedRoom(r.name)}<Icon name="lock" size={14} />{/if}
      <button onclick={() => hideRoom(r.name, !off)} title={off ? t('maison.pieces.afficher') : t('maison.pieces.masquer')} aria-label={off ? t('maison.pieces.afficher') : t('maison.pieces.masquer')}><Icon name={off ? 'eye' : 'eye-off'} size={15} /></button>
      {#if asking === r.name}
        <button class="danger" onclick={() => remove(r.name)}>{t('maison.pieces.confirmer')}</button>
        <button onclick={() => (asking = null)}>{t('commun.annuler')}</button>
      {:else}
        <button onclick={() => (asking = r.name)} title={t('maison.pieces.supprimer')} aria-label={t('maison.pieces.supprimer')}><Icon name="trash" size={15} /></button>
      {/if}
    </div>
  {/if}
{/snippet}

{#snippet detail()}
  {#if chosen}
    <div class="detail-head">
      <h2>{chosen.name}</h2>
      {#if chosen.temp != null}<span class="heat num">{num(chosen.temp, 1)}°{#if chosen.humidity != null}<small>{num(chosen.humidity)} %</small>{/if}</span>{/if}
      {#if !wide}<button class="h-round" onclick={closeSheet} aria-label={t('commun.fermer')}><Icon name="close" size={20} /></button>{/if}
    </div>
    {#if chosen.room}
      <RoomDetail room={chosen} sensors />
    {:else}
      <p class="muted">{t('maison.pieces.vide')} <a href="#/equipements">{t('maison.pieces.rattacher')}</a></p>
    {/if}
  {/if}
{/snippet}

<div class="pieces">
  <header>
    <div>
      <h1 class="page-title">{t('maison.pieces.titre')}</h1>
      <p class="page-sub">{t('maison.pieces.sous_titre')}</p>
    </div>
    <button class="edit" class:active={editing} onclick={() => (editing = !editing)}>
      <Icon name={editing ? 'check' : 'pencil'} size={16} />{editing ? t('maison.pieces.terminer') : t('maison.pieces.gerer')}
    </button>
  </header>

  {#if editing}
    <p class="hint">{t('maison.perso.aide')}</p>
    <form class="add" onsubmit={add}>
      <input bind:value={draft} maxlength="40" placeholder={t('maison.pieces.nom_nouvelle')} aria-label={t('maison.pieces.nom_nouvelle')} />
      <button type="submit" disabled={!draft.trim()}><Icon name="plus" size={16} />{t('maison.pieces.ajouter')}</button>
      <a href="#/equipements">{t('maison.pieces.rattacher')}</a>
    </form>
  {/if}

  <div class="split">
    <nav class="list" aria-label={t('maison.pieces.titre')}>
      {#each floors as floor (floor.id)}
        {#if floors.length > 1 && floor.name}<h2 class="floor">{floor.name}</h2>{/if}
        <div class="tiles"><Sortable list="pieces" items={floor.rooms} id={(r) => r.name} label={(r) => r.name} {editing} hideable={false} item={tile} extra={manage} /></div>
      {/each}
      {#if loose.length}<div class="tiles"><Sortable list="pieces" items={loose} id={(r) => r.name} label={(r) => r.name} {editing} hideable={false} item={tile} extra={manage} /></div>{/if}
    </nav>

    {#if wide}
      <section class="card detail">{@render detail()}</section>
    {/if}
  </div>
</div>

{#if !wide && chosen}
  <button class="scrim" aria-label={t('commun.fermer')} onclick={closeSheet}></button>
  <aside class="sheet" role="dialog" aria-modal="true" aria-label={chosen.name}>{@render detail()}</aside>
{/if}

<style>
  .pieces {
    display: grid;
    gap: 18px;
  }

  header {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    justify-content: space-between;
    gap: 12px;
  }

  .edit,
  .add button,
  .manage button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 0;
    border-radius: 999px;
    padding: 8px 14px;
    background: var(--surface-3);
    color: var(--ink);
    font-size: 13.5px;
    font-weight: 620;
  }

  .edit.active {
    background: var(--warm);
    color: #2d2108;
  }

  .hint {
    color: var(--ink-2);
    font-size: 14px;
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
    background: var(--surface);
    color: var(--ink);
    font: inherit;
  }

  .add button:disabled {
    opacity: 0.5;
  }

  .add a,
  .muted a {
    color: var(--warm-ink);
    font-size: 13.5px;
    font-weight: 620;
  }

  .split {
    display: grid;
    grid-template-columns: minmax(280px, 400px) minmax(0, 1fr);
    gap: 18px;
    align-items: start;
  }

  .list {
    display: grid;
    gap: 10px;
    min-width: 0;
  }

  .floor {
    margin-top: 6px;
    color: var(--ink-2);
    font-size: 13px;
    font-weight: 650;
  }

  .tiles {
    --sort-cols: repeat(2, minmax(0, 1fr));
    --sort-gap: 8px;
  }

  .manage {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 4px;
    margin-top: 4px;
  }

  .manage button {
    padding: 5px 9px;
    font-size: 12.5px;
  }

  .manage .danger {
    background: var(--alert);
    color: #fff;
  }

  .detail-head {
    display: flex;
    align-items: center;
    gap: 14px;
    margin-bottom: 6px;
  }

  .detail-head h2 {
    flex: 1;
    min-width: 0;
    font-family: var(--serif);
    font-size: 34px;
    font-weight: 380;
    letter-spacing: -0.015em;
    line-height: 1.1;
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

  .scrim {
    position: fixed;
    inset: 0;
    z-index: 30;
    border: 0;
    background: rgb(5 8 18 / 42%);
    cursor: default;
  }

  .sheet {
    position: fixed;
    z-index: 31;
    inset: auto 0 0;
    max-height: 88dvh;
    overflow-y: auto;
    padding: 20px 16px calc(22px + env(safe-area-inset-bottom));
    border-radius: 26px 26px 0 0;
    background: rgb(13 17 28 / 92%);
    backdrop-filter: blur(26px);
    color: var(--ink);
  }

  @media (max-width: 899px) {
    .split {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>

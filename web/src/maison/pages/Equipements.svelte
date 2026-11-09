<script>
  import { hub, roomList, roomOf, nameOf, deviceKind, hidden, reachable, roomNamed, home, driverName, isSmoke, isCoverGroup } from '../lib/home.svelte.js';
  import { layout, addRoom, setDeviceRoom, setDeviceName, onHome, showOnHome } from '../lib/layout.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from '../ui/Icon.svelte';
  import RfxPanel from '../ui/RfxPanel.svelte';
  import CoverGroupEditor from '../ui/CoverGroupEditor.svelte';
  import { newAlert } from '../lib/auto.svelte.js';
  import Sortable from '../ui/Sortable.svelte';
  import Personalize from '../ui/Personalize.svelte';
  import { arranged } from '../lib/order.svelte.js';
  import { onMount } from 'svelte';

  const KIND_ICON = {
    garage: 'gate',
    light: 'light',
    group: 'light-group',
    plug: 'power',
    cover: 'home-roof',
    climate: 'air-conditioner',
    tv: 'remote',
    speaker: 'speaker',
    camera: 'cctv',
    printer: 'printer3d',
    sensor: 'info',
  };
  const FILTERS = [
    { id: 'all', kinds: null },
    { id: 'none', kinds: null },
    { id: 'lights', kinds: ['light', 'group'] },
    { id: 'plugs', kinds: ['plug'] },
    { id: 'climate', kinds: ['climate', 'cover'] },
    { id: 'media', kinds: ['tv', 'speaker'] },
    { id: 'cameras', kinds: ['camera'] },
    { id: 'sensors', kinds: ['sensor', 'printer'] },
  ];

  let query = $state('');
  let filter = $state('all');
  /** grouped by where they come from (`source`) or where they are (`room`) */
  let by = $state('room');
  /** the groups opened, as `by:group`: all start folded, a line each */
  let opened = $state(new Set());

  /** Two columns that stack on their own: a group opened on one side
   *  leaves no hole on the other. Reading order stays row by row. */
  let wide = $state(true);
  onMount(() => {
    const media = matchMedia('(min-width: 761px)');
    wide = media.matches;
    const update = (e) => (wide = e.matches);
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  });
  let editing = $state(false);
  const VIEW_KEY = 'moli-equipements-ciel';
  try {
    const kept = JSON.parse(localStorage.getItem(VIEW_KEY) ?? '{}');
    if (['source', 'room', 'name', 'state'].includes(kept.by)) by = kept.by;
    if (Array.isArray(kept.opened)) opened = new Set(kept.opened);
  } catch {
    // Private window: the choice lasts the visit.
  }
  function keepView() {
    try {
      localStorage.setItem(VIEW_KEY, JSON.stringify({ by, opened: [...opened] }));
    } catch {
      // Same.
    }
  }
  function setBy(next) {
    by = next;
    keepView();
  }
  /** Searching or filtering opens everything, without forgetting what was open. */
  const looking = $derived(query.trim() !== '' || filter !== 'all');
  function folded(key, open) {
    if (looking || opened.has(key) === open) return;
    const next = new Set(opened);
    if (open) next.add(key);
    else next.delete(key);
    opened = next;
    keepView();
  }
  /** the device getting a brand new room */
  let creating = $state(null);
  let draft = $state('');

  const rooms = $derived.by(() => {
    const names = new Set([...roomList().map((r) => r.name), ...(home.config?.rooms ?? []).map((r) => r.name), ...layout.extraRooms]);
    return [...names].sort((a, b) => a.localeCompare(b));
  });

  const source = (d) => d.id.split(':')[0];
  /** A bridge's own interface (Zigbee2MQTT…), for what Moli does not do: pairing, device settings. */
  const webOf = (src) => hub.drivers?.find((x) => x.instance === src)?.web;
  /** A 433 MHz transceiver: its pairing and settings are Moli's own panel. */
  const isRfx = (src) => hub.drivers?.find((x) => x.instance === src)?.kind === 'rfxtrx';
  let rfx = $state(null);
  /** The groups of shutters, and the one being edited (`new`: a new one). */
  const coverGroups = $derived(devices.filter((x) => isCoverGroup(x.d)));
  const hasShutters = $derived(devices.some((x) => x.kind === 'cover' && !isCoverGroup(x.d)));
  let editingGroup = $state(null);
  /** Their own section, which has what their row needs: shown once. */
  const groupsSection = $derived((filter === 'all' || filter === 'climate') && !query && hasShutters);
  /** « Zigbee », not « z2m »: the catalog's name for where they come from. */
  const sourceName = (src) => {
    const driver = hub.drivers?.find((x) => x.instance === src);
    return driver && driver.kind !== '?' ? driverName(driver) : src;
  };
  const norm = (s) => String(s ?? '').normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();

  const devices = $derived(
    Object.values(hub.devices)
      .map((d) => ({ d, kind: deviceKind(d), room: roomOf(d), name: nameOf(d.id) }))
      .sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' })),
  );

  const shown = $derived.by(() => {
    const f = FILTERS.find((x) => x.id === filter);
    const q = norm(query);
    return devices.filter(
      (x) =>
        (filter === 'none' ? !x.room : !f.kinds || f.kinds.includes(x.kind)) &&
        (!q || norm(`${x.name} ${x.room ?? ''} ${source(x.d)} ${x.d.model ?? ''}`).includes(q)),
    );
  });

  /** Grouped by where they come from (Zigbee, Hue…), the largest first, or
   *  by room in the alphabet, the ones in no room last. */
  const groups = $derived.by(() => {
    const map = new Map();
    for (const x of shown) {
      if (by === 'source' && groupsSection && isCoverGroup(x.d)) continue;
      const k = by === 'room' ? (x.room ?? '') : by === 'name' ? 'all' : by === 'state' ? (onHome(x.d.id) ? 'on' : 'off') : source(x.d);
      if (!map.has(k)) map.set(k, []);
      map.get(k).push(x);
    }
    const list = [...map.entries()];
    if (by === 'state') return list.sort((a, b) => (a[0] === 'off') - (b[0] === 'off'));
    if (by === 'room') return list.sort((a, b) => (!a[0]) - (!b[0]) || a[0].localeCompare(b[0]));
    return list.sort((a, b) => b[1].length - a[1].length || a[0].localeCompare(b[0]));
  });

  /** Every bridge with its own interface: outside the « par origine » view,
   *  where each group carries its link, they are listed once under the bar. */
  const webBridges = $derived((hub.drivers ?? []).filter((x) => x.web));
  /** Bridges with an interface but no device yet: where pairing starts. */
  const bridges = $derived(by !== 'source' ? [] : (hub.drivers ?? []).filter((x) => x.web && !devices.some((d) => source(d.d) === x.instance)));

  const unplaced = $derived(devices.filter((x) => !x.room).length);

  const groupName = (src) => (by === 'room' ? src || t('maison.equipements.sans_piece') : by === 'name' ? t('maison.equipements.f_all') : by === 'state' ? t(`maison.ciel.etat_${src}`) : sourceName(src));

  async function choose(id, value) {
    if (value === '__new__') {
      creating = id;
      draft = '';
      return;
    }
    await setDeviceRoom(id, value || null);
  }

  /** the device being renamed, and what is typed */
  let renaming = $state(null);
  let nameDraft = $state('');

  function startRename(x) {
    renaming = x.d.id;
    nameDraft = x.name;
  }

  async function rename(x) {
    if (renaming !== x.d.id) return;
    renaming = null;
    const name = nameDraft.trim();
    if (name === x.name) return;
    await setDeviceName(x.d.id, name);
  }

  async function create(e) {
    e.preventDefault();
    const name = addRoom(draft);
    const id = creating;
    creating = null;
    if (name && id) await setDeviceRoom(id, roomNamed(name));
  }
</script>

{#snippet onHomeBox(x)}
  <label class="show" title={t('maison.ciel.case_aide', { name: x.name })}>
    <input type="checkbox" checked={onHome(x.d.id)} onchange={(e) => showOnHome(x.d.id, e.currentTarget.checked)} aria-label={t('maison.ciel.case_aide', { name: x.name })} />
    <span>{t('maison.ciel.case')}</span>
  </label>
{/snippet}

{#snippet roomPicker(x)}
  {#if creating === x.d.id}
    <form onsubmit={create}>
      <!-- svelte-ignore a11y_autofocus -->
      <input bind:value={draft} maxlength="40" placeholder={t('maison.pieces.nom_nouvelle')} aria-label={t('maison.pieces.nom_nouvelle')} autofocus />
      <button type="submit" disabled={!draft.trim()} aria-label={t('maison.pieces.ajouter')}><Icon name="check" size={16} /></button>
      <button type="button" onclick={() => (creating = null)} aria-label={t('commun.annuler')}><Icon name="close" size={16} /></button>
    </form>
  {:else}
    <select class:none={!x.room} value={x.room ?? ''} onchange={(e) => choose(x.d.id, e.currentTarget.value)} aria-label={t('maison.equipements.piece_de', { name: x.name })}>
      <option value="">{t('maison.equipements.aucune')}</option>
      {#each rooms as r (r)}<option value={r}>{r}</option>{/each}
      {#if x.room && !rooms.includes(x.room)}<option value={x.room}>{x.room}</option>{/if}
      <option value="__new__">{t('maison.equipements.nouvelle')}</option>
    </select>
  {/if}
{/snippet}

<div class="equipements">
  <header class="head">
    <div>
      <h1 class="page-title">{t('maison.equipements.titre')}</h1>
      <p class="page-sub">{t('maison.equipements.sous_titre', { count: devices.length })}</p>
    </div>
    <Personalize bind:editing lists={['equipements-source', 'equipements-room', 'equipements-state']} />
  </header>

  {#if editing}<p class="hint">{t('maison.perso.aide_groupes')}</p>{/if}

  <div class="bar">
    <label class="search">
      <Icon name="search" size={16} />
      <input bind:value={query} placeholder={t('maison.equipements.chercher')} aria-label={t('maison.equipements.chercher')} />
    </label>
    <div class="chips" role="group" aria-label={t('maison.equipements.filtrer')}>
      {#each FILTERS as f (f.id)}
        <button class:on={filter === f.id} aria-pressed={filter === f.id} onclick={() => (filter = f.id)}>
          {t(`maison.equipements.f_${f.id}`)}{#if f.id === 'none' && unplaced} <small>{unplaced}</small>{/if}
        </button>
      {/each}
    </div>
    <div class="chips by" role="radiogroup" aria-label={t('maison.equipements.ranger')}>
      {#each ['source', 'room', 'name', 'state'] as b (b)}
        <button role="radio" aria-checked={by === b} class:on={by === b} onclick={() => setBy(b)}>{t(`maison.equipements.par_${b}`)}</button>
      {/each}
    </div>
  </div>

  {#if by !== 'source' && webBridges.length}
    <nav class="bridges" aria-label={t('maison.equipements.interfaces')}>
      <span>{t('maison.equipements.interfaces')}</span>
      {#each webBridges as b (b.instance)}
        <a href={b.web} target="_blank" rel="noopener noreferrer" title={t('maison.equipements.interface_de', { name: driverName(b) })}>{driverName(b)} ↗</a>
      {/each}
    </nav>
  {/if}

  {#if groupsSection}
    <details class="group" open={looking || opened.has('cover-groups')} ontoggle={(e) => folded('cover-groups', e.currentTarget.open)}>
      <summary>
        <h2>
          {t('maison.groupes_stores.titre')} <small>{coverGroups.length}</small>
          <button class="web" onclick={(e) => { e.preventDefault(); editingGroup = 'new'; }}>{t('maison.groupes_stores.creer')}</button>
        </h2>
      </summary>
      <ul>
        {#each coverGroups as x (x.d.id)}
          <li class:off={!reachable(x.d.id)}>
            <span class="glyph"><Icon name="shutter" size={20} /></span>
            <span class="who">
              <b>{x.name}</b>
              <small>{(x.d.members ?? []).map((m) => nameOf(m)).join(' · ')}</small>
            </span>
            <button type="button" class="edit" onclick={() => (editingGroup = x.d.id)}>{t('maison.groupes_stores.modifier_court')}</button>
            {@render onHomeBox(x)}
            {@render roomPicker(x)}
          </li>
        {:else}
          <li>
            <span class="glyph"><Icon name="shutter" size={20} /></span>
            <span class="who"><b>{t('maison.groupes_stores.vide')}</b><small>{t('maison.groupes_stores.vide_aide')}</small></span>
          </li>
        {/each}
      </ul>
    </details>
  {/if}

  {#snippet groupBlock([src, list])}
    <details class="group" open={!editing && (looking || opened.has(`${by}:${src}`))} ontoggle={(e) => !editing && folded(`${by}:${src}`, e.currentTarget.open)}>
      <summary>
      <h2>
        {groupName(src)} <small>{list.length}</small>
        {#if by === 'source' && webOf(src)}<a class="web" href={webOf(src)} target="_blank" rel="noopener noreferrer" title={t('maison.equipements.interface_de', { name: sourceName(src) })}>{t('maison.equipements.interface')} ↗</a>{/if}
        {#if by === 'source' && isRfx(src)}<button class="web" onclick={() => (rfx = src)}>{t('maison.equipements.interface')}</button>{/if}
      </h2>
      </summary>
      <ul>
        {#each list as x (x.d.id)}
          <li class:off={!reachable(x.d.id)}>
            <span class="glyph"><Icon name={isSmoke(x.d) ? 'smoke-detector' : (KIND_ICON[x.kind] ?? 'info')} size={20} /></span>
            <span class="who">
              {#if renaming === x.d.id}
                <!-- svelte-ignore a11y_autofocus -->
                <input
                  class="rename"
                  bind:value={nameDraft}
                  maxlength="60"
                  autofocus
                  aria-label={t('maison.equipements.renommer', { name: x.name })}
                  onblur={() => rename(x)}
                  onkeydown={(e) => {
                    if (e.key === 'Enter') e.currentTarget.blur();
                    if (e.key === 'Escape') renaming = null;
                  }}
                />
              {:else}
                <b>{x.name}<button type="button" class="pen" title={t('maison.equipements.renommer', { name: x.name })} aria-label={t('maison.equipements.renommer', { name: x.name })} onclick={() => startRename(x)}>✎</button></b>
              {/if}
              <small>
                {isCoverGroup(x.d) ? t('maison.groupes_stores.modele') : [x.d.manufacturer, x.d.model].filter(Boolean).join(' ') || x.d.id}
                {#if !reachable(x.d.id)} · {t('maison.equipements.hors_ligne')}{/if}
                {#if hidden(x.d.id)} · {t('maison.equipements.masque')}{/if}
              </small>
            </span>
            <button type="button" class="bell" title={t('maison.equipements.alerte', { name: x.name })} aria-label={t('maison.equipements.alerte', { name: x.name })} onclick={() => newAlert(x.d.id)}><Icon name="bell" size={16} /></button>
            {@render onHomeBox(x)}
            {@render roomPicker(x)}
          </li>
        {/each}
      </ul>
    </details>
  {/snippet}

  {#if groups.length}
    {#if wide && !editing}
      {@const ordered = arranged(`equipements-${by}`, groups, ([src]) => src || '-')}
      <div class="columns">
        {#each [0, 1] as side (side)}
          <div class="column">
            {#each ordered.filter((_, i) => i % 2 === side) as g (g[0] || '-')}{@render groupBlock(g)}{/each}
          </div>
        {/each}
      </div>
    {:else}
      <div class="groups">
      <Sortable list="equipements-{by}" items={groups} id={([src]) => src || '-'} label={([src]) => groupName(src)} {editing} hideable={false} item={groupBlock} />
      </div>
    {/if}
  {:else if !bridges.length}
    <p class="empty">{t('maison.equipements.rien')}</p>
  {/if}

  {#if filter === 'all' && !query}
    {#each bridges as b (b.instance)}
      <section class="group">
        <h2>
          {driverName(b)} <small>0</small>
          <a class="web" href={b.web} target="_blank" rel="noopener noreferrer" title={t('maison.equipements.interface_de', { name: driverName(b) })}>{t('maison.equipements.interface')} ↗</a>
        </h2>
        <ul>
          <li>
            <span class="glyph"><Icon name="info" size={20} /></span>
            <span class="who"><b>{t('maison.equipements.pont_vide')}</b><small>{t('maison.equipements.pont_vide_aide')}</small></span>
          </li>
        </ul>
      </section>
    {/each}
  {/if}

  {#if rfx}<RfxPanel instance={rfx} onclose={() => (rfx = null)} />{/if}
  {#if editingGroup}<CoverGroupEditor id={editingGroup === 'new' ? null : editingGroup} onclose={() => (editingGroup = null)} />{/if}

  <nav class="more" aria-label={t('maison.equipements.autres_vues')}>
    <span>{t('maison.equipements.autres_vues')}</span>
    <a href="#/plan">{t('commun.nav.plan')}</a>
    <a href="#/carte">{t('commun.nav.carte')}</a>
    <a href="#/salon">{t('commun.nav.salon')}</a>
    <a href="#/dehors">{t('commun.nav.dehors')}</a>
    <a href="#/impression">{t('commun.nav.impression')}</a>
    <a href="#/parler">{t('commun.nav.parler')}</a>
    <a href="#/infra">{t('commun.nav.infra')}</a>
  </nav>
</div>

<style>
  .equipements {
    display: grid;
    /* A long line must not widen the page past a phone's screen. */
    grid-template-columns: minmax(0, 1fr);
    gap: 20px;
  }

  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    justify-content: space-between;
    gap: 12px;
  }

  .hint {
    color: var(--ink-2);
    font-size: 14px;
  }

  .bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
  }

  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
    min-width: 200px;
    max-width: 340px;
    padding: 8px 14px;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--ink-3);
  }

  .search input {
    flex: 1;
    min-width: 0;
    border: 0;
    outline: 0;
    background: transparent;
    font: inherit;
    color: var(--ink);
  }

  .chips {
    display: flex;
    gap: 6px;
    overflow-x: auto;
    scrollbar-width: none;
    min-width: 0;
  }

  .chips button {
    flex: none;
    border: 0;
    border-radius: 999px;
    padding: 7px 13px;
    font: inherit;
    font-size: 13px;
    font-weight: 650;
    background: var(--surface-2);
    color: var(--ink-3);
    cursor: pointer;
  }

  .chips.by {
    margin-left: auto;
  }

  .chips button.on {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  /* A group: one pane of glass, two side by side, open or folded (an open
     one keeps its column, so nothing moves around it). */
  .columns {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
    align-items: start;
  }

  .column {
    display: grid;
    gap: 8px;
    min-width: 0;
  }

  .groups {
    --sort-cols: repeat(2, minmax(0, 1fr));
    --sort-gap: 8px;
  }

  @media (max-width: 760px) {
    .groups {
      --sort-cols: minmax(0, 1fr);
    }
  }

  details.group {
    padding: 13px 16px;
    border-radius: 16px;
    background: var(--surface);
    box-shadow: var(--shadow);
  }

  details.group > summary h2 {
    margin: 0;
  }

  details.group[open] > summary h2 {
    margin-bottom: 8px;
  }

  details.group > summary {
    cursor: pointer;
    list-style: none;
  }

  details.group > summary::-webkit-details-marker {
    display: none;
  }

  details.group > summary h2::before {
    content: '›';
    display: inline-block;
    width: 14px;
    font-size: 16px;
    line-height: 1;
    transition: transform 0.15s;
  }

  details.group[open] > summary h2::before {
    transform: rotate(90deg);
  }

  .group h2 {
    margin: 0 0 8px;
    font-size: 13px;
    font-weight: 750;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ink-2);
  }

  .bell {
    flex: none;
    width: 34px;
    height: 34px;
    display: grid;
    place-items: center;
    border: 0;
    border-radius: 50%;
    background: none;
    color: var(--ink-3);
    cursor: pointer;
  }

  .bell:hover {
    background: var(--surface-2);
    color: var(--warm-ink);
  }

  .group h2 .web {
    float: right;
    border: 0;
    padding: 0;
    background: none;
    font-family: inherit;
    cursor: pointer;
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0;
    text-transform: none;
    color: var(--warm-ink);
    text-decoration: none;
  }

  .group h2 .web:hover {
    text-decoration: underline;
  }

  .edit {
    flex: none;
    min-height: 34px;
    padding: 6px 14px;
    border: 0;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--ink-2);
    font: inherit;
    font-size: 13px;
    font-weight: 650;
    cursor: pointer;
  }

  small {
    font-weight: 600;
    letter-spacing: 0;
    color: var(--ink-3);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    border-radius: 18px;
    background: var(--surface);
    overflow: hidden;
  }

  li {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 14px;
    border-top: 1px solid var(--line);
  }

  li:first-child {
    border-top: 0;
  }

  li.off .who {
    opacity: 0.55;
  }

  .glyph {
    flex: none;
    width: 36px;
    height: 36px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: var(--surface-2);
    color: var(--ink-2);
  }

  .who {
    flex: 1;
    min-width: 0;
    display: grid;
  }

  .who b,
  .who small {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .who small {
    font-size: 12px;
  }

  select,
  form input {
    flex: none;
    width: 190px;
    max-width: 46vw;
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 7px 12px;
    font: inherit;
    font-size: 14px;
    background: var(--surface);
    color: var(--ink);
  }

  .rename {
    width: 100%;
    border: 1px solid var(--line);
    border-radius: var(--r-sm);
    padding: 4px 8px;
    font: inherit;
    font-weight: 650;
    background: var(--surface);
    color: var(--ink);
  }

  .pen {
    border: 0;
    background: none;
    color: var(--ink-3);
    cursor: pointer;
    font-size: 14px;
    padding: 0 6px;
  }

  .pen:hover {
    color: var(--ink);
  }

  select.none {
    color: var(--ink-3);
    border-style: dashed;
  }

  form {
    display: flex;
    gap: 6px;
  }

  form button {
    display: grid;
    place-items: center;
    width: 34px;
    border: 0;
    border-radius: 50%;
    background: var(--surface-2);
    color: var(--ink-2);
    cursor: pointer;
  }

  .empty {
    color: var(--ink-3);
  }

  .bridges {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 6px 16px;
    font-size: 13px;
    color: var(--ink-3);
  }

  .bridges a {
    font-weight: 700;
    color: var(--warm-ink);
    text-decoration: none;
  }

  .bridges a:hover {
    text-decoration: underline;
  }

  .more {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 16px;
    font-size: 13px;
    color: var(--ink-3);
  }

  .more a {
    font-weight: 650;
    color: var(--ink-2);
  }

  @media (max-width: 760px) {
    li {
      flex-wrap: wrap;
    }

    select,
    form input {
      width: 100%;
      max-width: none;
    }

    form {
      flex: 1 0 100%;
    }
  }

  .show {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    font-weight: 600;
    color: var(--ink-2);
    cursor: pointer;
  }

  .show input {
    width: 18px;
    height: 18px;
    accent-color: var(--good);
    cursor: pointer;
  }
</style>

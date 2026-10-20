<script>
  import { hub, roomList, roomOf, nameOf, deviceKind, hidden, reachable, roomNamed, home } from '../lib/home.svelte.js';
  import { layout, addRoom, setDeviceRoom, onDashboard, showOnDashboard } from '../lib/layout.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from '../ui/Icon.svelte';

  const KIND_ICON = {
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
  /** the device getting a brand new room */
  let creating = $state(null);
  let draft = $state('');

  const rooms = $derived.by(() => {
    const names = new Set([...roomList().map((r) => r.name), ...(home.config?.rooms ?? []).map((r) => r.name), ...layout.extraRooms]);
    return [...names].sort((a, b) => a.localeCompare(b));
  });

  const source = (d) => d.id.split(':')[0];
  const norm = (s) => String(s ?? '').normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();

  const devices = $derived(
    Object.values(hub.devices)
      .map((d) => ({ d, kind: deviceKind(d), room: roomOf(d), name: nameOf(d.id) }))
      .sort((a, b) => a.name.localeCompare(b.name)),
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

  /** Grouped by where they come from (hue, tuya…), the largest first. */
  const groups = $derived.by(() => {
    const by = new Map();
    for (const x of shown) {
      const k = source(x.d);
      if (!by.has(k)) by.set(k, []);
      by.get(k).push(x);
    }
    return [...by.entries()].sort((a, b) => b[1].length - a[1].length || a[0].localeCompare(b[0]));
  });

  const unplaced = $derived(devices.filter((x) => !x.room).length);

  async function choose(id, value) {
    if (value === '__new__') {
      creating = id;
      draft = '';
      return;
    }
    await setDeviceRoom(id, value || null);
  }

  async function create(e) {
    e.preventDefault();
    const name = addRoom(draft);
    const id = creating;
    creating = null;
    if (name && id) await setDeviceRoom(id, roomNamed(name));
  }
</script>

<div class="equipements">
  <header>
    <h1 class="page-title">{t('maison.equipements.titre')}</h1>
    <p class="page-sub">{t('maison.equipements.sous_titre', { count: devices.length })}</p>
  </header>

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
  </div>

  {#each groups as [src, list] (src)}
    <section class="group">
      <h2>{src} <small>{list.length}</small></h2>
      <ul>
        {#each list as x (x.d.id)}
          <li class:off={!reachable(x.d.id)}>
            <span class="glyph"><Icon name={KIND_ICON[x.kind] ?? 'info'} size={20} /></span>
            <span class="who">
              <b>{x.name}</b>
              <small>
                {[x.d.manufacturer, x.d.model].filter(Boolean).join(' ') || x.d.id}
                {#if !reachable(x.d.id)} · {t('maison.equipements.hors_ligne')}{/if}
                {#if hidden(x.d.id)} · {t('maison.equipements.masque')}{/if}
              </small>
            </span>
            {#if x.kind === 'climate'}
              <label class="show" title={t('maison.equipements.afficher_dashboard', { name: x.name })}>
                <input type="checkbox" checked={onDashboard(x.d.id)} onchange={(e) => showOnDashboard(x.d.id, e.currentTarget.checked)} aria-label={t('maison.equipements.afficher_dashboard', { name: x.name })} />
                <span>{t('maison.equipements.dashboard')}</span>
              </label>
            {/if}
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
          </li>
        {/each}
      </ul>
    </section>
  {:else}
    <p class="empty">{t('maison.equipements.rien')}</p>
  {/each}

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
    gap: 20px;
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

  .chips button.on {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .group h2 {
    margin: 0 0 8px;
    font-size: 13px;
    font-weight: 750;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ink-2);
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

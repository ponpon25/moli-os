<script>
  import { hub, value, act, nameOf, savePref, note } from '../lib/home.svelte.js';
  import { onMount } from 'svelte';
  import { ROW, dash, setTile, tileOf } from '../lib/layout.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from '../ui/Icon.svelte';
  import PlayerCard from '../ui/PlayerCard.svelte';
  import { has, list } from '../lib/speakers.js';

  // Every Sonos room, and the groups they make. A group plays one thing:
  // its leader says what, the others follow.
  const everyRoom = $derived(
    Object.values(hub.devices)
      .filter((d) => d.manufacturer === 'Sonos')
      .sort((a, b) => nameOf(a.id).localeCompare(nameOf(b.id))),
  );

  // How the page looks: kept on this device, like the rooms' « Aa / icônes ».
  const MODES = {
    full: { size: 'l', sections: { volumes: true, join: true, library: true, progress: true }, ambient: true },
    balanced: { size: 'm', sections: { volumes: true, join: true, library: true, progress: true }, ambient: true },
    compact: { size: 's', sections: { volumes: true, join: false, library: false, progress: true }, ambient: true },
  };
  // A tile's default width, in twelfths of the page, by size; each tile can
  // then be pulled to its own (kept by the house, like the dashboard's tiles).
  const WIDTH = { s: 6, m: 6, l: 12 };
  const DEFAULT = { ...MODES.compact, hidden: [] };
  const KEY = 'enceintes-look';
  function readLook() {
    try {
      const kept = JSON.parse(localStorage.getItem(KEY) ?? 'null') ?? {};
      return { ...DEFAULT, ...kept, sections: { ...DEFAULT.sections, ...(kept.sections ?? {}) }, hidden: Array.isArray(kept.hidden) ? kept.hidden : [] };
    } catch {
      return { ...DEFAULT, sections: { ...DEFAULT.sections } };
    }
  }
  let look = $state(readLook());
  const keep = () => savePref(KEY, JSON.stringify(look));
  function setMode(name) {
    const m = MODES[name];
    look.size = m.size;
    look.sections = { ...m.sections };
    look.ambient = m.ambient;
    keep();
  }
  const mode = $derived(Object.keys(MODES).find((k) => MODES[k].size === look.size && Object.keys(MODES[k].sections).every((s) => MODES[k].sections[s] === look.sections[s])) ?? null);
  function toggleRoom(id, shownNow) {
    look.hidden = shownNow ? look.hidden.filter((x) => x !== id) : [...look.hidden, id];
    keep();
  }
  let customizing = $state(false);
  const rooms = $derived(everyRoom.filter((d) => !look.hidden.includes(d.id)));

  const groups = $derived.by(() => {
    const byLeader = new Map();
    for (const d of rooms) {
      const lead = value(d.id, 'group_leader') ?? d.native_name;
      if (!byLeader.has(lead)) byLeader.set(lead, []);
      byLeader.get(lead).push(d);
    }
    const order = tileOf('enceintes:order', { list: [] }).list;
    const rank = (key) => (order.includes(key) ? order.indexOf(key) : order.length);
    return [...byLeader.entries()]
      .map(([lead, members]) => {
        const leader = members.find((m) => m.native_name === lead) ?? members[0];
        // Leader first, then the others by name.
        const ordered = [leader, ...members.filter((m) => m !== leader)];
        return { key: lead, leader: leader.id, members: ordered };
      })
      .sort((a, b) => rank(a.key) - rank(b.key) || a.key.localeCompare(b.key));
  });

  // ---- the arrangement: width, height and order of the tiles ----------------------
  const GAP = 20;
  let gridWidth = $state(1200);
  /** 12 columns on a wide screen, 6 on a tablet, one on a phone (like the dashboard). */
  const cols = $derived(gridWidth >= 1000 ? 12 : gridWidth >= 620 ? 6 : 1);
  const spanOf = (w) => (cols === 12 ? w : cols === 6 ? Math.min(6, Math.max(3, Math.ceil(w / 2))) : 1);
  const gid = (g) => `enceintes:${g.key}`;
  const tileFor = (g) => tileOf(gid(g), { w: WIDTH[look.size] });

  let sizing = $state(null);
  function startResize(e, g) {
    e.preventDefault();
    e.stopPropagation();
    const unit = (gridWidth + GAP) / cols;
    const tile = tileFor(g);
    const [x0, y0] = [e.clientX, e.clientY];
    const span0 = spanOf(tile.w);
    const rows0 = tile.rows || Math.ceil((e.currentTarget.parentElement.offsetHeight + GAP) / ROW);
    const apply = (ev) => {
      const span = cols === 1 ? 1 : Math.min(cols, Math.max(cols === 12 ? 2 : 3, Math.round(span0 + (ev.clientX - x0) / unit)));
      const rows = Math.max(6, Math.round(rows0 + (ev.clientY - y0) / ROW));
      const w = cols === 12 ? span : span * 2;
      sizing = { key: g.key, w, rows };
      setTile(gid(g), { w: cols === 1 ? tile.w : w, rows }, false);
    };
    const end = () => {
      removeEventListener('pointermove', apply);
      removeEventListener('pointerup', end);
      removeEventListener('pointercancel', end);
      setTile(gid(g), {});
      sizing = null;
    };
    addEventListener('pointermove', apply);
    addEventListener('pointerup', end);
    addEventListener('pointercancel', end);
  }
  /** Back to the tile's own height. */
  const autoHeight = (g) => setTile(gid(g), { rows: 0 });

  let movingKey = $state(null);
  function startDrag(e, g) {
    if (e.button > 0) return;
    e.preventDefault();
    movingKey = g.key;
    const move = (ev) => {
      const over = document.elementFromPoint(ev.clientX, ev.clientY)?.closest('[data-gkey]');
      const target = over?.getAttribute('data-gkey');
      if (!target || target === movingKey) return;
      const keys = groups.map((x) => x.key);
      // Moving down it lands after the tile it is over, moving up before it.
      const to = keys.indexOf(target);
      keys.splice(keys.indexOf(movingKey), 1);
      keys.splice(to, 0, movingKey);
      setTile('enceintes:order', { list: keys }, false);
    };
    const end = () => {
      removeEventListener('pointermove', move);
      removeEventListener('pointerup', end);
      removeEventListener('pointercancel', end);
      setTile('enceintes:order', {});
      movingKey = null;
    };
    addEventListener('pointermove', move);
    addEventListener('pointerup', end);
    addEventListener('pointercancel', end);
  }

  function resetTiles() {
    dash.tiles = Object.fromEntries(Object.entries(dash.tiles).filter(([id]) => !id.startsWith('enceintes:')));
    savePref('maison-dashboard', JSON.stringify(dash.tiles));
  }

  // Apple Music: one search bar for the page. The hub searches Apple on behalf
  // of the chosen group (its answer lands on that group's leader), and a result
  // plays there now, next, or at the end of its queue.
  let term = $state('');
  // Where the music goes is chosen, never guessed (a room may be asleep, a
  // film may be on): the choice is kept on this device.
  let targetKey = $state(
    (() => {
      try {
        return localStorage.getItem('enceintes-target');
      } catch {
        return null;
      }
    })(),
  );
  let submitted = $state('');
  let busy = $state(false);
  const searchable = $derived(groups.some((g) => has(g.leader, 'search')));
  const target = $derived(groups.find((g) => g.key === targetKey && has(g.leader, 'search')) ?? null);
  // Looking things up plays nothing: it may go through any speaker (the answer
  // lands on it). Only playing needs the speaker to have been chosen.
  const lookup = $derived(target ?? groups.find((g) => has(g.leader, 'search')) ?? null);
  const KINDS = ['song', 'album', 'artist', 'playlist', 'station'];

  // The person's own Apple Music (library, recents, recommendations): they sign
  // in to Apple once, here, in this page (MusicKit); Moli keeps the token.
  let account = $state({ configured: false, connected: false });
  let musicReady = $state(false);
  const SHELVES = ['recommendations', 'recent', 'playlists', 'albums'];
  const sendHeaders = { 'content-type': 'application/json', 'x-moli-origin': 'ui' };
  async function loadAccount() {
    try {
      const res = await fetch('/api/integrations/apple-music');
      if (res.ok) account = await res.json();
    } catch {
      /* the hub is away: nothing to show */
    }
  }
  async function loadMusicKit() {
    if (!window.MusicKit) {
      await new Promise((resolve, reject) => {
        const tag = document.createElement('script');
        tag.src = 'https://js-cdn.music.apple.com/musickit/v3/musickit.js';
        tag.async = true;
        tag.onerror = () => reject(new Error('MusicKit'));
        document.addEventListener('musickitloaded', resolve, { once: true });
        document.head.appendChild(tag);
      });
    }
    return window.MusicKit;
  }
  // Ready before the click: a browser lets the sign-in window open only
  // straight from a tap, not after waiting for scripts.
  async function prepareAccount() {
    try {
      const res = await fetch('/api/integrations/apple-music/developer-token', { headers: sendHeaders });
      if (!res.ok) return;
      const { token } = await res.json();
      const MusicKit = await loadMusicKit();
      await MusicKit.configure({ developerToken: token, app: { name: 'Moli', build: '1' } });
      musicReady = true;
    } catch {
      musicReady = false;
    }
  }
  async function connectAccount() {
    try {
      const userToken = await window.MusicKit.getInstance().authorize();
      const res = await fetch('/api/integrations/apple-music', { method: 'PUT', headers: sendHeaders, body: JSON.stringify({ user_token: userToken }) });
      if (!res.ok) throw new Error((await res.json().catch(() => ({}))).error ?? res.statusText);
      await loadAccount();
    } catch (err) {
      note(t('enceintes.apple.echec', { error: err?.message ?? String(err) }), 'error');
    }
  }
  async function disconnectAccount() {
    await fetch('/api/integrations/apple-music', { method: 'DELETE', headers: sendHeaders }).catch(() => {});
    await loadAccount();
  }
  onMount(loadAccount);
  $effect(() => {
    if (account.configured && !account.connected && !musicReady) prepareAccount();
  });
  async function browse(what) {
    if (!lookup) return;
    submitted = `browse:${what}`;
    term = '';
    busy = true;
    await act(`${lookup.leader}/browse`, what, t(`enceintes.apple.${what}`));
    busy = false;
  }
  const shelf = $derived(submitted.startsWith('browse:') ? submitted.slice(7) : null);
  const found = $derived.by(() => {
    if (!lookup || !submitted || value(lookup.leader, 'results_for') !== submitted) return null;
    const hits = list(lookup.leader, 'results');
    return KINDS.map((kind) => ({ kind, hits: hits.filter((h) => h.kind === kind) })).filter((x) => x.hits.length);
  });
  async function runSearch() {
    const q = term.trim();
    if (!q || !lookup) return;
    submitted = q;
    busy = true;
    await act(`${lookup.leader}/search`, q, t('enceintes.rechercher'));
    busy = false;
  }
  const playHit = (hit, mode) =>
    !target
      ? note(t('enceintes.choisir_avant'), 'error')
      : act(`${target.leader}/play`, JSON.stringify({ kind: hit.kind, id: hit.id, title: hit.title, mode }), t('enceintes.lancer', { name: hit.title }));
  const groupName = (g) => g.members.map((m) => nameOf(m.id)).join(' + ');
  function clearSearch() {
    term = '';
    submitted = '';
  }

</script>

<div class="enceintes">
  <header class="head">
    <div>
      <h1 class="page-title">{t('enceintes.titre')}</h1>
      <p class="page-sub">{t('enceintes.sous_titre')}</p>
    </div>
    <button class="chip tune" class:cool={customizing} aria-expanded={customizing} onclick={() => (customizing = !customizing)}>
      <Icon name="tune" size={16} />{t('enceintes.perso.titre')}
    </button>
  </header>

  {#if customizing}
    <section class="card perso">
      <div class="row">
        <span class="label">{t('enceintes.perso.mode')}</span>
        {#each Object.keys(MODES) as m (m)}
          <button class="chip" class:cool={mode === m} aria-pressed={mode === m} onclick={() => setMode(m)}>{t(`enceintes.perso.mode_${m}`)}</button>
        {/each}
      </div>
      <div class="row">
        <span class="label">{t('enceintes.perso.taille')}</span>
        {#each ['s', 'm', 'l'] as size (size)}
          <button class="chip" class:cool={look.size === size} aria-pressed={look.size === size} onclick={() => { look.size = size; keep(); }}>{t(`enceintes.perso.taille_${size}`)}</button>
        {/each}
      </div>
      <div class="row">
        <span class="label">{t('enceintes.perso.afficher')}</span>
        {#each ['volumes', 'join', 'library', 'progress'] as part (part)}
          <label class="check"><input type="checkbox" checked={look.sections[part]} onchange={(e) => { look.sections[part] = e.currentTarget.checked; keep(); }} />{t(`enceintes.perso.${part}`)}</label>
        {/each}
        <label class="check"><input type="checkbox" checked={look.ambient} onchange={(e) => { look.ambient = e.currentTarget.checked; keep(); }} />{t('enceintes.perso.fond')}</label>
      </div>
      <div class="row">
        <span class="label">{t('enceintes.perso.pieces')}</span>
        {#each everyRoom as r (r.id)}
          {@const on = !look.hidden.includes(r.id)}
          <label class="check"><input type="checkbox" checked={on} onchange={() => toggleRoom(r.id, !on)} />{nameOf(r.id)}</label>
        {/each}
      </div>
      <div class="row">
        <button class="chip" onclick={() => { Object.assign(look, DEFAULT, { sections: { ...DEFAULT.sections }, hidden: [] }); keep(); resetTiles(); }}>{t('enceintes.perso.reinitialiser')}</button>
      </div>
    </section>
  {/if}

  {#if searchable}
    <section class="card applebar">
      <form onsubmit={(e) => { e.preventDefault(); runSearch(); }}>
        <label class="field">
          <Icon name="search" size={18} />
          <input type="search" placeholder={t('enceintes.rechercher_tout')} aria-label={t('enceintes.rechercher')} bind:value={term} />
          {#if term}<button type="button" class="clear-x" onclick={clearSearch} aria-label={t('enceintes.effacer')}><Icon name="close" size={16} /></button>{/if}
        </label>
        <label class="on">
          <span class="muted small">{t('enceintes.sur')}</span>
          <select value={target?.key ?? ''} onchange={async (e) => {
              targetKey = e.currentTarget.value;
              savePref('enceintes-target', targetKey);
              // The answer lives on the speaker asked: ask the new one.
              if (submitted.startsWith('browse:')) await browse(submitted.slice(7));
              else if (submitted) await runSearch();
            }} aria-label={t('enceintes.sur')}>
            <option value="" disabled>{t('enceintes.choisir')}</option>
            {#each groups.filter((g) => has(g.leader, 'search')) as g (g.key)}
              <option value={g.key}>{groupName(g)}</option>
            {/each}
          </select>
        </label>
        <button class="chip go" type="submit" disabled={busy || !term.trim() || !lookup}>{t('enceintes.chercher')}</button>
      </form>
      {#if !target}<p class="muted small hint">{t('enceintes.choisir_avant')}</p>{/if}
      {#if account.configured}
        <div class="shelves">
          {#if account.connected}
            {#each SHELVES as what (what)}
              <button class="chip" class:cool={shelf === what} disabled={busy || !lookup} onclick={() => browse(what)}>{t(`enceintes.apple.${what}`)}</button>
            {/each}
            <button class="link" onclick={disconnectAccount}>{t('enceintes.apple.deconnecter')}</button>
          {:else}
            <button class="chip go" disabled={!musicReady} onclick={connectAccount}>{musicReady ? t('enceintes.apple.connecter') : t('enceintes.apple.preparation')}</button>
            <span class="muted small">{t('enceintes.apple.aide')}</span>
          {/if}
        </div>
      {/if}
      {#if busy}
        <p class="muted small">{t('enceintes.recherche_en_cours')}</p>
      {:else if found}
        {#if shelf}<h3 class="shelf-title">{t(`enceintes.apple.${shelf}`)}</h3>{/if}
        {#each found as section (section.kind)}
          <div class="found">
            <h3>{t(`enceintes.kinds.${section.kind}`)}</h3>
            <ul class="cards">
              {#each section.hits as hit (hit.kind + hit.id)}
                <li>
                  <div class="art" class:round={hit.kind === 'artist'}>
                    {#if hit.art}<img src={hit.art} alt="" loading="lazy" referrerpolicy="no-referrer" />{/if}
                    <button class="big" onclick={() => playHit(hit, 'now')} title={t('enceintes.lire_maintenant')} aria-label={t('enceintes.lire_maintenant')}><Icon name="play" size={26} /></button>
                  </div>
                  <b>{hit.title}</b>
                  {#if hit.subtitle}<span class="muted small">{hit.subtitle}</span>{/if}
                  {#if hit.kind !== 'station'}
                    <span class="acts">
                      <button class="soft small" onclick={() => playHit(hit, 'next')} title={t('enceintes.lire_ensuite')} aria-label={t('enceintes.lire_ensuite')}><Icon name="queue-next" size={16} /></button>
                      <button class="soft small" onclick={() => playHit(hit, 'add')} title={t('enceintes.ajouter_file')} aria-label={t('enceintes.ajouter_file')}><Icon name="queue-add" size={16} /></button>
                    </span>
                  {/if}
                </li>
              {/each}
            </ul>
          </div>
        {/each}
        {#if !found.length}<p class="muted">{t('enceintes.aucun_resultat')}</p>{/if}
      {/if}
    </section>
  {/if}

  {#if !groups.length}
    <p class="muted">{t('enceintes.aucune')}</p>
  {/if}

  <div class="grid" class:editing={customizing} class:dragging={movingKey} style:--cols={cols} bind:clientWidth={gridWidth}>
    {#each groups as g (g.key)}
      {@const tile = tileFor(g)}
      <div
        class="widget"
        class:moving={movingKey === g.key}
        data-gkey={g.key}
        style:grid-column="span {spanOf(tile.w)}"
        style:min-height={tile.rows ? `${tile.rows * ROW}px` : null}
      >
        {#if customizing}
          <div class="bar" onpointerdown={(e) => startDrag(e, g)} role="presentation">
            <span class="grip" aria-hidden="true"><Icon name="drag" size={18} /></span>
            <b>{g.members.map((m) => nameOf(m.id)).join(' + ')}</b>
            <span class="size">{sizing?.key === g.key ? `${sizing.w}×${sizing.rows}` : tile.w + (tile.rows ? `×${tile.rows}` : '')}</span>
          </div>
        {/if}
      <PlayerCard members={g.members} {rooms} {look} groupKey={g.key} />
        {#if customizing && cols > 1}
          <button class="grow" onpointerdown={(e) => startResize(e, g)} ondblclick={() => autoHeight(g)} title={t('enceintes.perso.redimensionner')} aria-label={t('enceintes.perso.redimensionner')}>
            <Icon name="resize" size={18} />
          </button>
        {/if}
      </div>
    {/each}
  </div>
</div>

<style>
  .enceintes {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 20px;
  }

  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }

  .tune {
    border: 0;
  }

  .perso {
    display: grid;
    gap: 14px;
  }

  .perso .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 10px;
  }

  .perso .label {
    min-width: 11rem;
    font-weight: 650;
    color: var(--ink-2);
  }

  .perso .check {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    border-radius: 999px;
    background: var(--surface-2);
    font-size: 14px;
    cursor: pointer;
  }

  .perso input {
    accent-color: var(--warm);
  }

  /* The tiles: twelfths of the page, each pulled to the width (and height)
     the family wants. What is inside follows the tile's own width. */
  .grid {
    display: grid;
    grid-template-columns: repeat(var(--cols, 12), minmax(0, 1fr));
    grid-auto-flow: row dense;
    gap: 20px;
  }

  .widget {
    position: relative;
    display: flex;
    flex-direction: column;
    min-width: 0;
    container-type: inline-size;
  }

  .widget.moving {
    opacity: 0.55;
  }

  .grid.dragging {
    user-select: none;
    cursor: grabbing;
  }

  /* Arranging: a bar to move it by, a corner to pull it by. */
  .grid.editing .widget {
    outline: 2px dashed var(--line);
    outline-offset: 4px;
    border-radius: var(--r-lg);
  }

  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    margin-bottom: 6px;
    border-radius: 12px;
    background: var(--surface);
    box-shadow: var(--shadow);
    cursor: grab;
    touch-action: none;
    user-select: none;
  }

  .bar:active {
    cursor: grabbing;
  }

  .bar b {
    flex: 1;
    min-width: 0;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .bar .size {
    font-size: 12px;
    color: var(--ink-3);
    font-variant-numeric: tabular-nums;
  }

  /* The search bar and what it found. */
  .applebar {
    display: grid;
    gap: 14px;
  }

  .applebar form {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px;
  }

  .applebar .field {
    flex: 1 1 280px;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 14px;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--ink-3);
  }

  .applebar .field:focus-within {
    outline: 2px solid var(--cool);
  }

  .applebar input[type='search'] {
    flex: 1;
    min-width: 0;
    border: 0;
    background: none;
    padding: 13px 0;
    color: var(--ink);
    font: inherit;
    outline: none;
  }

  .applebar input[type='search']::-webkit-search-cancel-button {
    display: none;
  }

  .applebar .clear-x {
    border: 0;
    background: none;
    color: var(--ink-3);
    display: grid;
    place-items: center;
    padding: 4px;
  }

  .applebar .on {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }

  .applebar select {
    border: 0;
    border-radius: 999px;
    padding: 10px 14px;
    background: var(--surface-2);
    color: var(--ink);
    font: inherit;
  }

  .applebar .go {
    padding: 11px 20px;
    background: var(--ink);
    color: var(--bg);
  }

  .hint {
    margin: -4px 0 0 4px;
  }

  .shelves {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 10px;
  }

  .shelves button.chip {
    border: 0;
  }

  .shelves .link {
    border: 0;
    background: none;
    color: var(--ink-3);
    font-size: 13px;
    text-decoration: underline;
    margin-left: auto;
  }

  .shelf-title {
    font-size: 18px;
    font-weight: 650;
  }

  .found h3 {
    font-size: 15px;
    font-weight: 650;
    color: var(--ink-2);
    margin-bottom: 10px;
  }

  .cards {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 18px 16px;
  }

  .cards li {
    display: grid;
    gap: 2px;
    align-content: start;
    min-width: 0;
  }

  .cards b {
    margin-top: 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .cards .small {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .cards .art {
    position: relative;
    aspect-ratio: 1;
    border-radius: 16px;
    background: var(--surface-3);
    overflow: hidden;
    box-shadow: var(--shadow);
  }

  .cards .art.round {
    border-radius: 50%;
  }

  .cards .art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    transition: transform 0.4s var(--ease);
  }

  .cards li:hover .art img {
    transform: scale(1.05);
  }

  .cards .big {
    position: absolute;
    right: 10px;
    bottom: 10px;
    width: 46px;
    height: 46px;
    border: 0;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--ink);
    color: var(--bg);
    box-shadow: 0 8px 22px -6px rgb(0 0 0 / 55%);
    opacity: 0;
    transform: translateY(8px);
    transition:
      opacity 0.2s var(--ease),
      transform 0.2s var(--ease);
  }

  .cards li:hover .big,
  .cards .big:focus-visible {
    opacity: 1;
    transform: none;
  }

  @media (hover: none) {
    .cards .big {
      opacity: 1;
      transform: none;
    }
  }

  .cards .acts {
    display: flex;
    gap: 6px;
    margin-top: 6px;
  }

  .grow {
    position: absolute;
    right: 8px;
    bottom: 8px;
    z-index: 3;
    width: 34px;
    height: 34px;
    border: 0;
    border-radius: 10px;
    display: grid;
    place-items: center;
    background: var(--ink);
    color: var(--bg);
    cursor: nwse-resize;
    touch-action: none;
    box-shadow: var(--shadow-lift);
  }

  .small {
    font-size: 13px;
  }

  button.soft {
    width: var(--soft);
    height: var(--soft);
    background: var(--surface-2);
    color: var(--ink-2);
  }

  button.soft.small {
    width: 36px;
    height: 36px;
  }

  button.soft:disabled {
    opacity: 0.4;
  }

  button.chip {
    border: 0;
  }

  button.chip:disabled {
    opacity: 0.5;
  }
  .widget > :global(.player) {
    flex: 1;
  }
</style>

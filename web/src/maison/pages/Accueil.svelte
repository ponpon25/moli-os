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
  import { ROW, setTile, saveDashboard, resetDashboard, onDashboard } from '../lib/layout.svelte.js';
  import { catalogue, showTile, isClim, isDev, available } from '../lib/tiles.svelte.js';
  import DeviceCard from '../ui/DeviceCard.svelte';
  import SensorCard from '../ui/SensorCard.svelte';
  import SpeakerCard from '../ui/SpeakerCard.svelte';
  import PlayerCard from '../ui/PlayerCard.svelte';
  import { has } from '../lib/speakers.js';
  import CoverTile from '../ui/CoverTile.svelte';
  import PrinterCard from '../ui/PrinterCard.svelte';
  import PaperPrinterCard from '../ui/PaperPrinterCard.svelte';
  import Remote from '../ui/Remote.svelte';
  import { deviceKind } from '../lib/home.svelte.js';
  import { isPaperPrinter } from '../lib/paper.js';

  const cfg = $derived(home.config ?? {});

  // ---- the arrangement (kept on this device) -------------------------------------
  let editing = $state(false);
  /** How a speaker's player shows on the dashboard: volume and progress, no
   *  grouping or library (those are the Speakers page's). */
  const PLAYER_LOOK = { sections: { volumes: true, join: false, library: false, progress: true }, ambient: true };
  let gridEl = $state();
  let gridWidth = $state(1200);
  let gridHeight = $state(0);
  let stageEl = $state();

  /** The grid's columns follow the room there is: 12 on a wide screen, 6 on a
   *  tablet, one on a phone. A tile keeps its width in twelfths and takes the
   *  nearest span of the grid it is drawn in. */
  const cols = $derived(gridWidth >= 1000 ? 12 : gridWidth >= 620 ? 6 : 1);
  const spanOf = (w) => (cols === 12 ? w : cols === 6 ? Math.min(6, Math.max(3, Math.ceil(w / 2))) : 1);
  const GAP = 20;
  /** The arranging view draws the real tiles at half size (so they keep their
   *  proportions); a row of it is then half a row, in px. */
  /** How much the arranging view shrinks the dashboard. The tiles are laid out
   *  at their true width and the whole grid is then scaled, so what is seen
   *  there is what the dashboard will be, only smaller. */
  const scale = $derived(!editing ? 1 : cols === 12 ? 0.6 : cols === 6 ? 0.75 : 1);

  // The room there is is the stage's; the grid's own height is what the
  // scaled view takes.
  $effect(() => {
    if (!stageEl || !gridEl) return;
    const measure = () => {
      gridWidth = stageEl.clientWidth;
      gridHeight = gridEl.offsetHeight;
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(stageEl);
    ro.observe(gridEl);
    addEventListener('resize', measure);
    return () => {
      ro.disconnect();
      removeEventListener('resize', measure);
    };
  });

  /** The order while a tile is being moved on a narrow screen (null the rest of the time). */
  let live = $state(null);
  let dragId = $state(null);
  /** where the tile being moved (or resized) asks to be: the others make room */
  let drag = $state(null);
  let prio = $state(null);
  /** the floating copy under the finger */
  let floater = $state(null);
  /** the size shown while a tile is being resized */
  let sizing = $state(null);
  /** the rows a tile takes by itself (what it shows, measured) */
  let auto = $state({});

  /** Measures a tile that has no height of its own (0: it shows nothing). */
  function fit(node, id) {
    const ro = new ResizeObserver(() => {
      const h = node.offsetHeight;
      const rows = h === 0 ? (node.classList.contains('preview') ? 3 : 0) : Math.max(2, Math.ceil((h + GAP) / ROW));
      if (auto[id] !== rows) auto[id] = rows;
    });
    ro.observe(node);
    return { destroy: () => ro.disconnect() };
  }

  const tiles = $derived(catalogue().filter((d) => isClim(d.id) || (isDev(d.id) ? !!device(d.device) : available(d.id) && (d.id !== 'lights' || shownLights.length > 0) && (d.id !== 'ambiances' || !!mood))));

  // ---- where the tiles sit ---------------------------------------------------------
  // A grid of 12 columns and thin rows; a tile has its column and row (`x`,
  // `y`), its width and its rows. One that never had a place (or is new)
  // takes the first that fits, in the order it comes.
  const COLS = 12;
  const BIG = 1e6;
  const wideOf = (t) => Math.min(COLS, Math.max(1, t.w));
  const rowsOf = (t) => t.rows || auto[t.id] || 6;

  /** Where every tile ends up. Each asks for a place; they are taken by `y`
   *  then `x` (the one being moved first on a tie), and each sits as high as it
   *  can without covering one already placed. So a tile dropped on another
   *  pushes it down, and a hole is filled by what comes after. */
  function solve(items, first) {
    const order = items.slice().sort((a, b) => a.y - b.y || (a.id === first ? -1 : b.id === first ? 1 : 0) || a.x - b.x || a.seq - b.seq);
    const done = [];
    const at = new Map();
    for (const it of order) {
      /** the highest row where it sits at column `x` */
      const drop = (x) => {
        const free = (y) => done.every((p) => x >= p.x + p.w || x + it.w <= p.x || y >= p.y + p.r || y + it.r <= p.y);
        return [0, ...done.map((p) => p.y + p.r)].sort((m, n) => m - n).find(free);
      };
      let x = Math.min(Math.max(0, it.x), COLS - it.w);
      // No place of its own yet: the first that fits, from the top left.
      if (it.fresh) {
        let best = Infinity;
        for (let c = 0; c <= COLS - it.w; c++) if (drop(c) < best) [x, best] = [c, drop(c)];
      }
      const y = drop(x);
      done.push({ x, y, w: it.w, r: it.r });
      at.set(it.id, { x, y });
    }
    return at;
  }

  const hereTiles = $derived(tiles.filter((x) => x.on));
  const placed = $derived.by(() => {
    // A tile that shows nothing (no routine, say) takes no place.
    const items = hereTiles
      .filter((x) => editing || x.rows || auto[x.id] !== 0)
      .map((x, i) => ({ id: x.id, x: x.x ?? 0, y: x.y ?? BIG + i, w: wideOf(x), r: rowsOf(x), seq: i, fresh: x.y == null }));
    const mine = drag && items.find((i) => i.id === drag.id);
    if (mine) [mine.x, mine.y, mine.fresh] = [drag.x, drag.y, false];
    return solve(items, drag?.id ?? prio);
  });
  const shownTiles = $derived.by(() => {
    if (live) {
      const by = new Map(hereTiles.map((x) => [x.id, x]));
      return live.map((id) => by.get(id)).filter(Boolean);
    }
    const pos = (x) => placed.get(x.id) ?? { x: 0, y: BIG };
    return hereTiles.slice().sort((p, q) => pos(p).y - pos(q).y || pos(p).x - pos(q).x);
  });

  /** Keeps the places as they now are, for every tile. */
  function freeze(pos, save = true) {
    for (const [id, p] of pos) setTile(id, { x: p.x, y: p.y }, false);
    if (save) saveDashboard();
  }

  /** The tiles in this reading order, packed from the top (a narrow screen
   *  only moves tiles in a list). */
  function repack(ids) {
    const items = ids.map((id, i) => {
      const x = hereTiles.find((t) => t.id === id);
      return { id, x: 0, y: BIG + i, w: wideOf(x), r: rowsOf(x), seq: i, fresh: true };
    });
    freeze(solve(items, null));
  }

  function scroller(el) {
    for (let n = el.parentElement; n; n = n.parentElement) {
      const oy = getComputedStyle(n).overflowY;
      if ((oy === 'auto' || oy === 'scroll') && n.scrollHeight > n.clientHeight) return n;
    }
    return document.scrollingElement;
  }

  function startDrag(e, tile) {
    if (e.button > 0) return;
    e.preventDefault();
    const widget = e.currentTarget.closest('.widget');
    const box = widget.getBoundingClientRect();
    const [dx, dy] = [e.clientX - box.left, e.clientY - box.top];
    const scrolled = scroller(gridEl);
    const grid = cols === COLS;
    let [px, py] = [e.clientX, e.clientY];
    let raf = 0;
    dragId = tile.id;
    // Everything stays where it is, whatever the tile that moves does.
    freeze(placed, false);
    if (grid) drag = { id: tile.id, ...placed.get(tile.id) };
    else live = shownTiles.map((x) => x.id);
    floater = { label: tile.label, x: box.left, y: box.top, w: box.width, h: box.height };

    /** Asks for the place under the tile: on the grid, the cell its corner is
     *  over; on a narrow screen, the spot in the list. */
    const place = () => {
      floater = { ...floater, x: px - dx, y: py - dy };
      if (grid) {
        const g = gridEl.getBoundingClientRect();
        const pitch = (g.width + GAP * scale) / COLS;
        drag = {
          id: tile.id,
          x: Math.min(COLS - wideOf(tile), Math.max(0, Math.round((px - dx - g.left) / pitch))),
          y: Math.max(0, Math.round((py - dy - g.top) / (ROW * scale))),
        };
        return;
      }
      // The tile under the pointer, else the nearest one (below the last, say).
      let target = null;
      let best = Infinity;
      for (const el of gridEl.querySelectorAll('.widget')) {
        if (el.dataset.id === tile.id) continue;
        const r = el.getBoundingClientRect();
        const dist = Math.hypot(Math.max(r.left - px, 0, px - r.right), Math.max(r.top - py, 0, py - r.bottom));
        if (dist < best) [target, best] = [el, dist];
      }
      if (!target) return;
      const ids = live.filter((i) => i !== tile.id);
      const at = ids.indexOf(target.dataset.id);
      const r = target.getBoundingClientRect();
      const across = r.width > gridEl.getBoundingClientRect().width * 0.6;
      const after = across || py > r.bottom ? py - r.top > r.height / 2 : px - r.left > r.width / 2;
      const next = [...ids.slice(0, at + (after ? 1 : 0)), tile.id, ...ids.slice(at + (after ? 1 : 0))];
      if (next.join() !== live.join()) live = next;
    };
    // The page follows when the pointer nears its top or bottom edge.
    const tick = () => {
      const view = scrolled === document.scrollingElement ? { top: 0, bottom: innerHeight } : scrolled.getBoundingClientRect();
      const edge = 70;
      const speed = py < view.top + edge ? -(view.top + edge - py) / 4 : py > view.bottom - edge ? (py - (view.bottom - edge)) / 4 : 0;
      if (speed) {
        scrolled.scrollTop += speed;
        place();
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    const move = (ev) => {
      [px, py] = [ev.clientX, ev.clientY];
      place();
    };
    const end = (ev) => {
      removeEventListener('pointermove', move);
      removeEventListener('pointerup', end);
      removeEventListener('pointercancel', end);
      cancelAnimationFrame(raf);
      if (ev.type === 'pointerup') {
        if (grid && drag) freeze(placed);
        else if (live) repack(live);
      }
      dragId = live = floater = drag = null;
    };
    addEventListener('pointermove', move);
    addEventListener('pointerup', end);
    addEventListener('pointercancel', end);
  }

  function startResize(e, tile) {
    e.preventDefault();
    e.stopPropagation();
    const unit = (gridWidth + GAP) / cols;
    const [x0, y0] = [e.clientX, e.clientY];
    const span0 = spanOf(tile.w);
    const rows0 = tile.rows || auto[tile.id] || 6;
    prio = tile.id;
    freeze(placed, false);
    const apply = (ev) => {
      const span = cols === 1 ? 1 : Math.min(cols, Math.max(cols === 12 ? 2 : 3, Math.round(span0 + (ev.clientX - x0) / scale / unit)));
      const rows = Math.max(2, Math.round(rows0 + (ev.clientY - y0) / scale / ROW));
      const w = cols === 12 ? span : span * 2;
      sizing = { id: tile.id, w, rows };
      setTile(tile.id, { w: cols === 1 ? tile.w : w, rows }, false);
    };
    const end = () => {
      removeEventListener('pointermove', apply);
      removeEventListener('pointerup', end);
      removeEventListener('pointercancel', end);
      freeze(placed);
      prio = null;
      sizing = null;
    };
    addEventListener('pointermove', apply);
    addEventListener('pointerup', end);
    addEventListener('pointercancel', end);
  }
  /** Back to the tile's own height. */
  const autoHeight = (tile) => setTile(tile.id, { rows: 0 });
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
  const shownLights = $derived(lights.filter((l) => onDashboard(l.id)));
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
      list.push({ tone: 'warm', icon: 'bolt', text: t('maison.accueil.facture', { bill: num(bill) }), href: '#/reglages/energie' });
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

<div class="accueil" class:editing>
  <header class="head">
    <h1 class="page-title">{t('maison.dashboard.titre')}</h1>
    {#if editing}
      <button class="ghost" onclick={() => resetDashboard()}>{t('maison.dashboard.reinitialiser')}</button>
    {/if}
    <button class="edit" class:active={editing} onclick={() => (editing = !editing)}>
      <Icon name={editing ? 'check' : 'pencil'} size={16} />{editing ? t('maison.dashboard.terminer') : t('maison.dashboard.personnaliser')}
    </button>
  </header>
  {#if editing}<p class="hint">{t('maison.dashboard.aide')} <a href="#/equipements">{t('maison.dashboard.masquees')}</a></p>{/if}

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
      {:else if kind === 'plug'}
        <LightTile id={tile.device} name={tile.label} icon="power" />
      {:else if kind === 'cover'}
        <CoverTile id={tile.device} name={tile.label} />
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

  <div class="stage" class:arranging-stage={editing} bind:this={stageEl} style:height={editing && gridHeight ? `${gridHeight * scale + 26 + 180}px` : null}>
  <div class="grid" class:arranging={editing} style:--s={scale} style:grid-template-columns="repeat({cols}, minmax(0, 1fr))" style:transform={editing ? `scale(${scale})` : null} style:width={editing ? `${gridWidth}px` : null} bind:this={gridEl}>
    {#each shownTiles as tile (tile.id)}
      {#if tile.on}
        <div
          class="widget"
          class:off={!tile.on}
          class:moving={dragId === tile.id}
          data-id={tile.id}
          style:grid-column={cols === COLS ? `${(placed.get(tile.id)?.x ?? 0) + 1} / span ${wideOf(tile)}` : `span ${spanOf(tile.w)}`}
          style:grid-row={cols === COLS ? `${(placed.get(tile.id)?.y ?? 0) + 1} / span ${rowsOf(tile)}` : `span ${rowsOf(tile)}`}
          role="group"
          aria-label={tile.label}
        >
          {#if editing}
            <div class="bar" style:zoom={1 / scale} onpointerdown={(e) => startDrag(e, tile)} role="presentation">
              <span class="grip" aria-hidden="true"><Icon name="drag" size={18} /></span>
              <b>{tile.label}</b>
              <span class="size">{sizing?.id === tile.id ? `${sizing.w}×${sizing.rows}` : tile.w + (tile.rows ? `×${tile.rows}` : '')}</span>
              <button onpointerdown={(e) => e.stopPropagation()} onclick={() => showTile(tile, !tile.on)} title={tile.on ? t('maison.dashboard.masquer') : t('maison.dashboard.afficher')} aria-label={tile.on ? t('maison.dashboard.masquer') : t('maison.dashboard.afficher')}>
                <Icon name={tile.on ? 'eye' : 'eye-off'} size={16} />
              </button>
            </div>
            <div class="inner preview" class:auto={!tile.rows} use:fit={tile.id}>{@render body(tile)}</div>
          {:else}
            <div class="inner" class:auto={!tile.rows} use:fit={tile.id}>{@render body(tile)}</div>
          {/if}
          {#if editing && tile.on && cols > 1}
            <button class="grow" style:zoom={1 / scale} onpointerdown={(e) => startResize(e, tile)} ondblclick={() => autoHeight(tile)} title={t('maison.dashboard.redimensionner')} aria-label={t('maison.dashboard.redimensionner')}>
              <Icon name="resize" size={18} />
            </button>
          {/if}
        </div>
      {/if}
    {/each}
  </div>
  </div>
  {#if floater}
    <div class="floater" style:left="{floater.x}px" style:top="{floater.y}px" style:width="{floater.w}px" style:height="{floater.h}px">
      <Icon name="drag" size={18} /><b>{floater.label}</b>
    </div>
  {/if}
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

  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px;
  }

  .head .page-title {
    flex: 1 1 auto;
    margin: 0;
  }

  .edit,
  .ghost,
  .bar button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 0;
    border-radius: 999px;
    padding: 7px 14px;
    font: inherit;
    font-size: 13px;
    font-weight: 650;
    white-space: nowrap;
    background: var(--surface-2);
    color: var(--ink-2);
    cursor: pointer;
  }

  .edit.active {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .hint a {
    color: var(--warm-ink);
  }

  .hint {
    margin: -8px 0 0;
    font-size: 13px;
    color: var(--ink-3);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(12, minmax(0, 1fr));
    /* A tile that fits a hole higher up takes it: a hidden tile leaves no gap. */
    grid-auto-flow: dense;
    /* Thin rows: a tile takes as many as it needs, and tiles stack freely. */
    grid-auto-rows: 20px;
    gap: 0 20px;
    align-items: stretch;
  }

  .widget {
    position: relative;
    grid-column: span var(--span, 12);
    grid-row: span var(--rows, 6);
    display: grid;
    grid-template-rows: minmax(0, 1fr);
    min-width: 0;
    container-type: inline-size;
  }

  /* A card that shows nothing (no routine, say) takes no place, nor a gap. */
  .accueil:not(.editing) .widget:has(> .inner:empty) {
    display: none;
  }

  .inner {
    display: grid;
    gap: 20px;
    min-width: 0;
    /* the gap under the tile */
    margin-bottom: 20px;
  }

  /* A height of its own: what does not fit scrolls inside the tile instead of
     spilling over the next one. */
  .inner:not(.auto) {
    overflow: auto;
    scrollbar-width: none;
  }

  /* No height of its own: as tall as what it shows (which is measured). */
  .inner.auto {
    align-self: start;
  }

  .inner > :global(*) {
    min-width: 0;
  }

  .stage {
    min-width: 0;
  }

  /* Room above the first tab and below the last tile, to drop a tile there. */
  .stage.arranging-stage {
    padding-top: 26px;
    padding-bottom: 180px;
  }

  /* Arranging: the dashboard as it is, scaled down (the stage keeps the room
     it takes), each tile with a bar to take hold of it. */
  .arranging {
    transform-origin: 0 0;
  }

  /* The outline is the tile's own, not the gap under it. */
  .arranging .preview {
    outline: 2px dashed color-mix(in srgb, var(--ink-3) 55%, transparent);
    outline-offset: 4px;
    border-radius: 18px;
    min-height: 60px;
  }

  .arranging .widget.off {
    opacity: 0.45;
  }

  /* Hold the place of the tile being moved. */
  .arranging .widget.moving .preview {
    outline-color: var(--warm-ink);
    background: var(--warm-soft);
    opacity: 0.6;
  }

  .arranging .widget.moving .bar,
  .arranging .widget.moving .grow {
    visibility: hidden;
  }

  /* The tile itself: it shows what it is but does not act. */
  .preview {
    pointer-events: none;
    user-select: none;
  }

  .floater {
    position: fixed;
    z-index: 50;
    display: flex;
    align-items: center;
    gap: 8px;
    box-sizing: border-box;
    padding: 10px 12px;
    border: 2px solid var(--warm-ink);
    border-radius: 16px;
    background: var(--warm-soft);
    color: var(--warm-ink);
    font-size: 13px;
    box-shadow: 0 14px 34px rgb(0 0 0 / 0.35);
    pointer-events: none;
    align-items: flex-start;
  }

  /* A tab hung over the tile's top edge, so it hides nothing of the tile. */
  .bar {
    position: absolute;
    z-index: 2;
    top: 0;
    left: 10px;
    max-width: calc(100% - 20px);
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 4px 3px 8px;
    border-radius: 999px;
    border: 1px solid var(--line, color-mix(in srgb, var(--ink) 14%, transparent));
    background: var(--surface-2);
    box-shadow: 0 4px 14px rgb(0 0 0 / 0.25);
    transform: translateY(-65%);
    font-size: 13px;
    cursor: grab;
    touch-action: none;
    user-select: none;
  }

  .bar:active {
    cursor: grabbing;
  }

  .bar b {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .size {
    font-size: 11px;
    color: var(--ink-3);
    font-variant-numeric: tabular-nums;
  }

  .bar button {
    padding: 5px 9px;
  }

  .grip {
    display: grid;
    color: var(--ink-3);
  }

  .grow {
    position: absolute;
    z-index: 3;
    right: 8px;
    bottom: 8px;
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    box-shadow: 0 4px 14px rgb(0 0 0 / 0.35);
    border: 0;
    border-radius: 50%;
    background: linear-gradient(var(--warm-soft), var(--warm-soft)) var(--surface-2);
    color: var(--warm-ink);
    cursor: nwse-resize;
    touch-action: none;
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

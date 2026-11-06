<script>
  import { ROW } from '../lib/layout.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** A page of tiles the family arranges: « Personnaliser » lets each be moved,
   *  resized and hidden. The Dashboard and the Shutters page are both this.
   *
   *  `tiles`: every tile, shown or not: `id`, `label`, `on`, `w` (of 12), `rows`
   *  (0: as tall as what it shows), `x`, `y`. `board`: where the arrangement is
   *  kept, `set(id, patch, save)` and `save()`. `body(tile)`: what a tile shows.
   *  `hint`: said while arranging; `extra`: shown below it; `actions`: buttons
   *  beside the title. */
  let {
    title,
    sub = null,
    tiles,
    board,
    body,
    onhide,
    onreset,
    hint = null,
    extra = null,
    actions = null,
    editing = $bindable(false),
  } = $props();

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
    for (const [id, p] of pos) board.set(id, { x: p.x, y: p.y }, false);
    if (save) board.save();
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
      board.set(tile.id, { w: cols === 1 ? tile.w : w, rows }, false);
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
  const autoHeight = (tile) => board.set(tile.id, { rows: 0 });
</script>

<div class="board" class:editing>
  <header class="head">
    <div class="titles">
      <h1 class="page-title">{title}</h1>
      {#if sub}<p class="page-sub">{sub}</p>{/if}
    </div>
    {#if !editing && actions}{@render actions()}{/if}
    {#if editing}
      <button class="ghost" onclick={() => onreset()}>{t('maison.dashboard.reinitialiser')}</button>
    {/if}
    <button class="edit" class:active={editing} onclick={() => (editing = !editing)}>
      <Icon name={editing ? 'check' : 'pencil'} size={16} />{editing ? t('maison.dashboard.terminer') : t('maison.dashboard.personnaliser')}
    </button>
  </header>
  {#if editing && hint}<p class="hint">{@render hint()}</p>{/if}
  {#if editing && extra}{@render extra()}{/if}

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
              <button onpointerdown={(e) => e.stopPropagation()} onclick={() => onhide(tile)} title={t('maison.dashboard.masquer')} aria-label={t('maison.dashboard.masquer')}>
                <Icon name="eye" size={16} />
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
  .board {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 22px;
  }

  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px;
  }

  .head .page-title {
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

  .hint :global(a) {
    color: var(--warm-ink);
  }

  .hint {
    margin: -8px 0 0;
    font-size: 13px;
    color: var(--ink-3);
  }

  .titles {
    flex: 1 1 auto;
    min-width: 0;
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
  .grid:not(.arranging) .widget:has(> .inner:empty) {
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
</style>

<script>
  import { t } from '../../../../lib/i18n.svelte.js';
  import Icon from '../../../ui/Icon.svelte';
  import TileGrid from './TileGrid.svelte';

  /** A page of tiles the family arranges: « Personnaliser » lets each be moved,
   *  resized and hidden. The Dashboard and the Shutters page are both this.
   *
   *  `tiles`: every tile, shown or not: `id`, `label`, `on`, `w` (of 12), `rows`
   *  (0: as tall as what it shows), `x`, `y`. `board`: where the arrangement is
   *  kept: `set(id, patch, save)`, `save()`, and, with `sectionOf`, `rooms()`
   *  and `setRooms(yes)`. `body(tile)`: what a tile shows. `sectionOf(tile)`
   *  offers « Par pièce »: `{ key, title, order }` for a tile, each section
   *  then a grid of its own, with places (`gx`, `gy`) that leave the other
   *  arrangement alone. `hint`: said while arranging; `extra`: shown below it;
   *  `actions`: buttons beside the title. */
  let {
    title,
    sub = null,
    tiles,
    board,
    body,
    onhide,
    onreset,
    sectionOf = null,
    hint = null,
    extra = null,
    actions = null,
    editing = $bindable(false),
  } = $props();

  const byRoom = $derived(!!sectionOf && board.rooms());
  const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });

  const sections = $derived.by(() => {
    const shown = tiles.filter((x) => x.on);
    if (!byRoom) return [{ key: 'all', title: null, tiles: shown }];
    const map = new Map();
    for (const tile of shown) {
      const s = sectionOf(tile);
      if (!map.has(s.key)) map.set(s.key, { ...s, tiles: [] });
      map.get(s.key).tiles.push({ ...tile, x: tile.gx, y: tile.gy });
    }
    return [...map.values()].sort((a, b) => a.order - b.order || collator.compare(a.title ?? '', b.title ?? ''));
  });

  /** A section's places are not the page's. */
  const sectionBoard = $derived({
    set(id, { x, y, ...patch }, save = true) {
      board.set(id, { ...patch, ...(x == null ? {} : { gx: x }), ...(y == null ? {} : { gy: y }) }, save);
    },
    save: () => board.save(),
  });
</script>

<div class="board" class:editing>
  <header class="head">
    <div class="titles">
      <h1 class="page-title">{title}</h1>
      {#if sub}<p class="page-sub">{sub}</p>{/if}
    </div>
    {#if !editing && actions}{@render actions()}{/if}
    {#if sectionOf}
      <button class="ghost" class:active={byRoom} aria-pressed={byRoom} onclick={() => board.setRooms(!byRoom)}>
        <Icon name="light-group" size={16} />{t('maison.dashboard.par_piece')}
      </button>
    {/if}
    {#if editing}
      <button class="ghost" onclick={() => onreset()}>{t('maison.dashboard.reinitialiser')}</button>
    {/if}
    <button class="edit" class:active={editing} onclick={() => (editing = !editing)}>
      <Icon name={editing ? 'check' : 'pencil'} size={16} />{editing ? t('maison.dashboard.terminer') : t('maison.dashboard.personnaliser')}
    </button>
  </header>
  {#if editing && hint}<p class="hint">{@render hint()}</p>{/if}
  {#if editing && extra}{@render extra()}{/if}

  {#each sections as s, i (s.key)}
    <section class="section">
      {#if s.title}<h2>{s.title}</h2>{/if}
      <TileGrid tiles={s.tiles} board={byRoom ? sectionBoard : board} {body} {onhide} {editing} room={i === sections.length - 1 ? 180 : 20} />
    </section>
  {/each}
</div>

<style>
  .board {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 22px;
  }

  .section {
    min-width: 0;
  }

  h2 {
    margin: 0 0 10px;
    font-size: 13px;
    font-weight: 750;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ink-2);
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
  .ghost {
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

  .edit.active,
  .ghost.active {
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
</style>

<script>
  import { hub, deviceKind, hidden, isCoverGroup, nameOf, roomOf } from '../lib/home.svelte.js';
  import { storesBoard, STORES, resetStores } from '../lib/layout.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from '../ui/Icon.svelte';
  import ShutterCard from '../ui/ShutterCard.svelte';
  import CoverGroupEditor from '../ui/CoverGroupEditor.svelte';
  import TileBoard from '../ui/TileBoard.svelte';

  /** Every shutter of the house, arranged like the dashboard. Until the family
   *  arranges them: the groups first, then room by room, in no room last. */
  const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
  const WIDTH = 4;

  const all = $derived(Object.values(hub.devices).filter((d) => deviceKind(d) === 'cover' && !hidden(d.id)));
  const tiles = $derived(
    all
      .map((d) => ({ d, room: roomOf(d), group: isCoverGroup(d), label: nameOf(d.id) }))
      .sort((a, b) => b.group - a.group || !a.room - !b.room || collator.compare(a.room ?? '', b.room ?? '') || collator.compare(a.label, b.label))
      .map(({ d, label }) => ({ id: d.id, label, on: true, w: WIDTH, rows: 0, ...storesBoard.tiles[d.id] })),
  );
  const shutters = $derived(all.filter((d) => !isCoverGroup(d)).length);
  const away = $derived(tiles.filter((x) => !x.on));

  /** « Par pièce »: the groups, then the rooms, those in no room last. */
  function sectionOf(tile) {
    const d = hub.devices[tile.id];
    if (isCoverGroup(d)) return { key: '__groups', title: t('maison.groupes_stores.titre'), order: 0 };
    const room = roomOf(d);
    return room ? { key: room, title: room, order: 1 } : { key: '', title: t('maison.equipements.sans_piece'), order: 2 };
  }

  let creating = $state(false);
</script>

{#snippet body(tile)}<ShutterCard id={tile.id} />{/snippet}
{#snippet hint()}{t('maison.stores.aide')}{/snippet}
{#snippet extra()}
  {#if away.length}
    <div class="away">
      <span>{t('maison.stores.masques')}</span>
      {#each away as x (x.id)}
        <button onclick={() => STORES.set(x.id, { on: true })} aria-label={t('maison.stores.afficher', { name: x.label })}><Icon name="plus" size={14} />{x.label}</button>
      {/each}
    </div>
  {/if}
{/snippet}
{#snippet actions()}
  {#if shutters}
    <button class="create" onclick={() => (creating = true)}><Icon name="plus" size={18} />{t('maison.groupes_stores.creer')}</button>
  {/if}
{/snippet}

<TileBoard
  title={t('maison.stores.titre')}
  sub={t('maison.stores.sous_titre', { count: shutters })}
  {tiles}
  board={STORES}
  {body}
  {hint}
  {extra}
  {actions}
  {sectionOf}
  onhide={(tile) => STORES.set(tile.id, { on: false })}
  onreset={resetStores}
/>

{#if !all.length}<p class="empty">{t('maison.stores.aucun')}</p>{/if}
{#if creating}<CoverGroupEditor onclose={() => (creating = false)} />{/if}

<style>
  .create {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 0;
    border-radius: 999px;
    padding: 7px 14px;
    background: var(--warm-soft);
    color: var(--warm-ink);
    font: inherit;
    font-size: 13px;
    font-weight: 650;
    white-space: nowrap;
    cursor: pointer;
  }

  .away {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--ink-3);
  }

  .away button {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-height: 34px;
    padding: 5px 12px;
    border: 0;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--ink-2);
    font: inherit;
    font-size: 13px;
    font-weight: 650;
    cursor: pointer;
  }

  .empty {
    color: var(--ink-3);
  }
</style>

<script>
  import { hub, deviceKind, hidden, isCoverGroup, nameOf, roomOf } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from '../ui/Icon.svelte';
  import ShutterCard from '../ui/ShutterCard.svelte';
  import CoverGroupEditor from '../ui/CoverGroupEditor.svelte';

  /** Every shutter of the house in one place: the groups first, then the
   *  shutters room by room, those in no room last. */
  const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
  const byName = (a, b) => collator.compare(nameOf(a.id), nameOf(b.id));

  const all = $derived(Object.values(hub.devices).filter((d) => deviceKind(d) === 'cover' && !hidden(d.id)));
  const groups = $derived(all.filter(isCoverGroup).sort(byName));
  const rooms = $derived.by(() => {
    const map = new Map();
    for (const d of all.filter((x) => !isCoverGroup(x))) {
      const room = roomOf(d) ?? '';
      if (!map.has(room)) map.set(room, []);
      map.get(room).push(d);
    }
    return [...map.entries()]
      .map(([room, list]) => [room, list.sort(byName)])
      .sort((a, b) => !a[0] - !b[0] || collator.compare(a[0], b[0]));
  });
  const shutters = $derived(all.length - groups.length);

  let creating = $state(false);
</script>

<div class="stores">
  <header>
    <div>
      <h1 class="page-title">{t('maison.stores.titre')}</h1>
      <p class="page-sub">{t('maison.stores.sous_titre', { count: shutters })}</p>
    </div>
    {#if shutters}
      <button class="create" onclick={() => (creating = true)}><Icon name="plus" size={18} />{t('maison.groupes_stores.creer')}</button>
    {/if}
  </header>

  {#if groups.length}
    <section aria-labelledby="stores-groupes">
      <h2 id="stores-groupes">{t('maison.groupes_stores.titre')}</h2>
      <div class="grid">
        {#each groups as d (d.id)}<ShutterCard id={d.id} />{/each}
      </div>
    </section>
  {/if}

  {#each rooms as [room, list] (room)}
    <section>
      <h2>{room || t('maison.equipements.sans_piece')}</h2>
      <div class="grid">
        {#each list as d (d.id)}<ShutterCard id={d.id} />{/each}
      </div>
    </section>
  {:else}
    <p class="empty">{t('maison.stores.aucun')}</p>
  {/each}

  {#if creating}<CoverGroupEditor onclose={() => (creating = false)} />{/if}
</div>

<style>
  .stores {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 24px;
  }

  header {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }

  .create {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-height: 40px;
    padding: 8px 16px;
    border: 0;
    border-radius: 999px;
    background: var(--warm-soft);
    color: var(--warm-ink);
    font: inherit;
    font-size: 14px;
    font-weight: 650;
    cursor: pointer;
  }

  h2 {
    margin: 0 0 10px;
    font-size: 13px;
    font-weight: 750;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ink-2);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 320px), 1fr));
    gap: 16px;
  }

  .empty {
    color: var(--ink-3);
  }
</style>

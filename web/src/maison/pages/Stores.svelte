<script>
  // Every shutter of the house, at a glance: the groups first, then one line a
  // window, each set to the unit. A shutter's durations and a group's list are
  // behind its settings.
  import { hub, deviceKind, hidden, isCoverGroup, nameOf, roomOf } from '../lib/home.svelte.js';
  import { coversWord, shortName } from '../lib/accueil.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from '../ui/Icon.svelte';
  import CoverRow from '../ui/CoverRow.svelte';
  import CoverGroupEditor from '../ui/CoverGroupEditor.svelte';
  import Sortable from '../ui/Sortable.svelte';
  import Personalize from '../ui/Personalize.svelte';

  const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
  const all = $derived(Object.values(hub.devices).filter((d) => deviceKind(d) === 'cover' && !hidden(d.id)));
  const groups = $derived(all.filter(isCoverGroup).sort((a, b) => collator.compare(nameOf(a.id), nameOf(b.id))));
  const shutters = $derived(
    all
      .filter((d) => !isCoverGroup(d))
      .map((d) => ({ d, room: roomOf(d) }))
      .sort((a, b) => !a.room - !b.room || collator.compare(a.room ?? '', b.room ?? '') || collator.compare(nameOf(a.d.id), nameOf(b.d.id))),
  );
  /** The room is the title; what is left of the shutter's name, if anything. */
  const words = (x) => {
    if (!x.room) return { name: nameOf(x.d.id), sub: t('maison.equipements.sans_piece') };
    const rest = shortName(x.d, x.room);
    return { name: x.room, sub: rest.toLowerCase() === 'store' ? null : rest };
  };

  let creating = $state(false);
  let editing = $state(false);
  const LISTS = ['stores-groupes', 'stores'];
</script>

<div class="stores">
  <header>
    <div>
      <h1 class="page-title">{t('maison.stores.titre')}</h1>
      <p class="page-sub">{coversWord(shutters.map((x) => x.d))}</p>
    </div>
 <div class="acts">
    <Personalize bind:editing lists={LISTS} />
    {#if shutters.length}
      <button class="create" onclick={() => (creating = true)}><Icon name="plus" size={17} />{t('maison.groupes_stores.creer')}</button>
    {/if}
    </div>
  </header>

  {#if editing}<p class="hint">{t('maison.perso.aide')}</p>{/if}

  {#if groups.length}
    <section class="card list">
      <h2>{t('maison.groupes_stores.titre')}</h2>
      {#snippet groupRow(d)}<CoverRow {d} name={nameOf(d.id)} tune />{/snippet}
      <Sortable list="stores-groupes" items={groups} id={(d) => d.id} label={(d) => nameOf(d.id)} {editing} item={groupRow} />
    </section>
  {/if}

  {#if shutters.length}
    <section class="card list">
      <h2>{t('maison.accueil.par_piece')}</h2>
      {#snippet shutterRow(x)}
        {@const w = words(x)}
        <CoverRow d={x.d} name={w.name} sub={w.sub} tune />
      {/snippet}
      <Sortable list="stores" items={shutters} id={(x) => x.d.id} label={(x) => nameOf(x.d.id)} {editing} item={shutterRow} />
    </section>
  {/if}
</div>

{#if creating}
  <CoverGroupEditor onclose={() => (creating = false)} />
{/if}

<style>
  .stores {
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

  .create {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 9px 15px;
    border: 0;
    border-radius: 999px;
    background: var(--surface-3);
    color: var(--ink);
    font-weight: 620;
  }

  h2 {
    margin-bottom: 4px;
    color: var(--ink-3);
    font-size: 13px;
    font-weight: 620;
  }

  .list {
    --sort-cols: repeat(auto-fill, minmax(min(320px, 100%), 1fr));
    --sort-gap: 0 28px;
  }

  .acts {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .hint {
    color: var(--ink-2);
    font-size: 14px;
  }
</style>

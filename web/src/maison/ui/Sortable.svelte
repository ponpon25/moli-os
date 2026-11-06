<script>
  /** Things the family can put in the order it likes, and hide. Outside
   *  `editing` it is only a grid. While editing, what they hold sleeps: a
   *  mouse moves one from anywhere, a finger from its handle (so the page still
   *  scrolls), the arrow keys from the handle too. The columns come from the
   *  page: `--sort-cols` and `--sort-gap`. */
  import { flip } from 'svelte/animate';
  import { arranged, isHiddenIn, setHiddenIn, setOrder } from '../lib/order.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  let { list, items, id, label, editing = false, hideable = true, span = null, item, extra = null } = $props();

  // While moving: the order asked so far, saved when it is let go.
  let moving = $state(null);
  let draft = $state(null);

  const base = $derived(arranged(list, items, id, editing));
  const shown = $derived.by(() => {
    if (!draft) return base;
    const rank = new Map(draft.map((x, i) => [x, i]));
    return [...base].sort((a, b) => rank.get(id(a)) - rank.get(id(b)));
  });

  let box = $state();

  function start(e, x) {
    if (e.button > 0 || e.target.closest('[data-no-drag]')) return;
    if (e.pointerType === 'touch' && !e.target.closest('[data-grip]')) return;
    e.preventDefault();
    moving = id(x);
    draft = base.map(id);
    const follow = (ev) => {
      // Near the edges of the window, the page follows.
      if (ev.clientY < 70) scrollBy(0, -12);
      else if (ev.clientY > innerHeight - 110) scrollBy(0, 12);
      const over = document
        .elementsFromPoint(ev.clientX, ev.clientY)
        .map((el) => el.closest('[data-sort-id]'))
        .find((el) => el && box?.contains(el));
      const target = over?.dataset.sortId;
      if (!target || target === String(moving)) return;
      const next = draft.filter((v) => String(v) !== String(moving));
      const to = next.findIndex((v) => String(v) === target);
      const from = draft.findIndex((v) => String(v) === String(moving));
      const was = draft.findIndex((v) => String(v) === target);
      next.splice(from < was ? to + 1 : to, 0, moving);
      draft = next;
    };
    const end = (ev) => {
      removeEventListener('pointermove', follow);
      removeEventListener('pointerup', end);
      removeEventListener('pointercancel', end);
      if (ev.type === 'pointerup' && draft.join('\n') !== base.map(id).join('\n')) setOrder(list, draft);
      moving = null;
      draft = null;
    };
    addEventListener('pointermove', follow);
    addEventListener('pointerup', end);
    addEventListener('pointercancel', end);
  }

  function key(e, x) {
    const step = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 }[e.key];
    if (!step) return;
    e.preventDefault();
    const ids = base.map(id);
    const from = ids.indexOf(id(x));
    const to = from + step;
    if (to < 0 || to >= ids.length) return;
    ids.splice(from, 1);
    ids.splice(to, 0, id(x));
    setOrder(list, ids);
  }
</script>

<div class="sortable" class:editing bind:this={box}>
  {#each shown as x (id(x))}
    {@const off = isHiddenIn(list, id(x))}
    <div class="it" class:off class:moving={moving === id(x)} data-sort-id={id(x)} style:--span={span ? span(x) : 1} animate:flip={{ duration: moving ? 160 : 0 }}>
      <div class="body">
        <div class="held" inert={editing}>{@render item(x)}</div>
        {#if editing}
          <div class="cover" role="presentation" onpointerdown={(e) => start(e, x)}>
            <button class="grip" data-grip aria-label={t('maison.perso.deplacer', { name: label(x) })} title={t('maison.perso.deplacer', { name: label(x) })} onkeydown={(e) => key(e, x)}><Icon name="drag" size={18} /></button>
            {#if hideable}
              <button class="eye" data-no-drag aria-pressed={off} aria-label={off ? t('maison.perso.afficher', { name: label(x) }) : t('maison.perso.masquer', { name: label(x) })} title={off ? t('maison.perso.afficher', { name: label(x) }) : t('maison.perso.masquer', { name: label(x) })} onclick={() => setHiddenIn(list, id(x), !off)}><Icon name={off ? 'eye-off' : 'eye'} size={16} /></button>
            {/if}
          </div>
        {/if}
      </div>
      {#if extra}{@render extra(x)}{/if}
    </div>
  {/each}
</div>

<style>
  .sortable {
    display: grid;
    grid-template-columns: var(--sort-cols, minmax(0, 1fr));
    gap: var(--sort-gap, 8px);
  }

  .it {
    min-width: 0;
    grid-column: span var(--span, 1);
  }

  @media (max-width: 760px) {
    .it {
      grid-column: auto;
    }
  }

  .body {
    position: relative;
    height: 100%;
  }

  .held {
    height: 100%;
  }

  /* A strip above what it holds, for the handle and the eye. */
  .editing .held {
    height: auto;
    padding-top: 42px;
  }

  .cover {
    position: absolute;
    inset: 0;
    z-index: 2;
    border-radius: 16px;
    outline: 1.5px dashed rgb(255 255 255 / 40%);
    outline-offset: -2px;
    cursor: grab;
    user-select: none;
  }

  .moving .cover {
    cursor: grabbing;
    outline-style: solid;
    outline-color: #fff;
    background: rgb(255 255 255 / 12%);
  }

  .moving {
    z-index: 3;
    transform: scale(1.02);
  }

  .off .held {
    opacity: 0.35;
  }

  .grip,
  .eye {
    position: absolute;
    top: 6px;
    width: 34px;
    height: 34px;
    display: grid;
    place-items: center;
    border: 0;
    border-radius: 10px;
    background: rgb(10 16 32 / 72%);
    color: #fff;
  }

  .grip {
    left: 6px;
    cursor: grab;
    touch-action: none;
  }

  .eye {
    right: 6px;
  }
</style>

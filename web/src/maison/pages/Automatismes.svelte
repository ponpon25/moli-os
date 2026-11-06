<script>
  import { onMount } from 'svelte';
  import Icon from '../ui/Icon.svelte';
  import Orb from '../moli/Orb.svelte';
  import Editor from '../auto/Editor.svelte';
  import Sortable from '../ui/Sortable.svelte';
  import Personalize from '../ui/Personalize.svelte';
  import { home, hub, note, relative, nameOf, roomOf, device } from '../lib/home.svelte.js';
  import { api, asHuman, pending, liveRuns, newAlert, nodeText, splitPoint, CATALOG, CANCELLED } from '../lib/auto.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';

  /** « The n8n of the house »: what runs by itself, and a way to ask for
   *  more in plain words. */
  let { route = '' } = $props();

  const sub = $derived(route.split('/')[1] ?? '');

  let list = $state(null);
  let abilities = $state({});
  let recent = $state([]);
  let ask = $state('');
  let drafting = $state(false);
  let error = $state('');
  let editing = $state(false);

  async function refresh() {
    try {
      const r = await api.list();
      list = r.automations;
      abilities = r.abilities ?? {};
      recent = await api.runs(12);
      error = '';
    } catch (err) {
      error = err.message;
      list = [];
    }
  }

  onMount(() => {
    refresh();
    return liveRuns((ev) => {
      if (ev.type === 'run_ended' || ev.type === 'changed') refresh();
    });
  });

  $effect(() => {
    if (!sub) refresh();
  });

  // Icon and id of each idea; its words are `automatismes.idee.<id>.titre` and `.demande`.
  const IDEAS = [
    ['door-open', 'entree'],
    ['moon', 'minuit'],
    ['bolt', 'conso'],
    ['bell', 'sonnette'],
    ['sparkles', 'recap'],
    ['cctv', 'cour'],
  ];

  async function draft(request) {
    const text = (request ?? ask).trim();
    if (!text || drafting) return;
    drafting = true;
    try {
      const d = await api.draft(text, null);
      pending.draft = { name: d.name, mode: d.mode, graph: d.graph, note: d.note };
      ask = '';
      location.hash = '#/automatismes/nouveau';
    } catch (err) {
      note(err.status === 503 ? t('automatismes.moli_absent_zero') : t('automatismes.moli_erreur', { message: err.message }), 'error');
    } finally {
      drafting = false;
    }
  }

  async function toggle(item) {
    const a = item.automation;
    try {
      if (item.live) {
        await api.enabled(a.id, false);
        note(t('automatismes.liste.coupe_note', { nom: a.name }));
      } else {
        await asHuman(t('automatismes.activer_label', { nom: a.name }), () => api.approve(a.id, item.fingerprint));
        // Home Assistant's original still runs until someone switches it off.
        note(a.author === 'import' ? t('automatismes.liste.tourne_import', { nom: a.name }) : t('automatismes.liste.tourne_note', { nom: a.name }));
      }
    } catch (err) {
      if (err.message !== CANCELLED) note(err.message, 'error');
    }
    refresh();
  }

  function stateOf(item) {
    if (item.live) return ['live', t('automatismes.etat.live')];
    if (item.problems?.some((p) => p.level === 'error')) return ['bad', t('automatismes.etat.bad')];
    if (!item.approved) return ['draft', t('automatismes.etat.draft')];
    return ['off', t('automatismes.etat.off')];
  }

  const mine = $derived((list ?? []).filter((i) => i.automation.author !== 'import' || i.live));
  const imported = $derived((list ?? []).filter((i) => i.automation.author === 'import' && !i.live));

  const triggerIcon = (a) => CATALOG[a.graph.nodes.find((n) => CATALOG[n.type]?.family === 'trigger')?.type]?.icon ?? 'robot';
  const AUTHOR = { assistant: 'Moli', import: 'HA' };
  const isTrigger = (n) => CATALOG[n.type]?.family === 'trigger';

  /** An alert: one trigger about one device, then only messages. Shown
   *  with the device, in a line; anything else is a scenario. */
  function alertOf(a) {
    const triggers = a.graph.nodes.filter(isTrigger);
    const rest = a.graph.nodes.filter((n) => !isTrigger(n));
    if (triggers.length !== 1 || !rest.length || rest.some((n) => n.type !== 'notify')) return null;
    const trigger = triggers[0];
    const id = trigger.device || splitPoint(trigger.point ?? '')[0];
    if (!id || !hub.devices[id]) return null;
    const suffix = ` · ${nameOf(id)}`;
    return {
      device: id,
      trigger,
      label: a.name.endsWith(suffix) ? a.name.slice(0, -suffix.length) : a.name,
      channels: [...new Set(rest.flatMap((n) => n.channels ?? ['maison']))],
    };
  }

  // What an alert watches, by the point it reads.
  const ALERT_ICON = { smoke: 'fire', water_leak: 'water', watersensor_state: 'water', test: 'check', battery: 'battery-low', battery_percentage: 'battery-low', battery_low: 'battery-low', contact: 'door-open', doorcontact_state: 'door-open', occupancy: 'motion', motion: 'motion', doorbell: 'bell', temperature: 'thermometer', humidity: 'humidity' };
  const alertIcon = (al) => ALERT_ICON[splitPoint(al.trigger.point ?? '')[1]] ?? CATALOG[al.trigger.type]?.icon ?? 'bell';

  // Where the words go, as the house calls it.
  const messengers = $derived(Object.values(hub.devices).filter((d) => d.points.some((p) => p.key === 'notify' && p.access?.write)));
  const CHANNEL_ICON = { telegram: 'send', maison: 'home', voix: 'speaker', telephone: 'phone' };
  const channelName = (c) =>
    c === 'telegram' && messengers.length ? messengers.map((d) => nameOf(d.id)).join(', ') : t(`automatismes.inspecteur.canal.${c}`);

  const alertGroups = $derived.by(() => {
    const by = new Map();
    for (const item of mine) {
      const al = alertOf(item.automation);
      if (!al) continue;
      if (!by.has(al.device)) by.set(al.device, []);
      by.get(al.device).push({ item, al });
    }
    return [...by.entries()].map(([id, items]) => ({ id, items }));
  });
  const scenarios = $derived(mine.filter((i) => !alertOf(i.automation)));

  /** A scenario in a few words: what starts it, what it does. */
  function gist(a) {
    const when = a.graph.nodes.filter(isTrigger).map(nodeText).join(` ${t('automatismes.texte.ou')} `);
    const does = [];
    const devices = new Set();
    for (const n of a.graph.nodes) {
      if ((n.type === 'set' || n.type === 'toggle') && n.point) {
        const id = splitPoint(n.point)[0];
        if (!devices.has(id)) {
          devices.add(id);
          does.push({ icon: 'tune', text: nameOf(id) });
        }
      }
    }
    const said = a.graph.nodes.filter((n) => n.type === 'notify');
    for (const c of new Set(said.flatMap((n) => n.channels ?? ['maison']))) does.push({ icon: CHANNEL_ICON[c] ?? 'bell', text: channelName(c) });
    return { when, does, steps: a.graph.nodes.length };
  }

  /** the item whose « … » menu is open */
  let menu = $state(null);

  async function remove(item) {
    menu = null;
    const a = item.automation;
    if (!confirm(t('automatismes.editeur.supprimer_question', { nom: a.name }))) return;
    try {
      await asHuman(t('automatismes.editeur.supprimer_label', { nom: a.name }), () => api.remove(a.id));
      note(t('automatismes.editeur.supprime'));
    } catch (err) {
      if (err.message !== CANCELLED) note(err.message, 'error');
    }
    refresh();
  }
</script>

{#snippet more(item)}
  {@const a = item.automation}
  <div class="more">
    <button class="dots" onclick={(e) => (e.stopPropagation(), (menu = menu === a.id ? null : a.id))} aria-label={t('automatismes.liste.actions', { nom: a.name })} aria-expanded={menu === a.id}><Icon name="dots" size={18} /></button>
    {#if menu === a.id}
      <div class="menu" role="menu">
        <a role="menuitem" href="#/automatismes/{a.id}"><Icon name="pencil" size={16} />{t('automatismes.liste.modifier')}</a>
        <button role="menuitem" class="danger" onclick={() => remove(item)}><Icon name="trash" size={16} />{t('automatismes.editeur.supprimer')}</button>
      </div>
    {/if}
  </div>
{/snippet}

{#snippet sw(item)}
  <button class="switch" class:on={item.live} onclick={() => toggle(item)} aria-label={item.live ? t('automatismes.liste.couper') : t('automatismes.liste.activer')} aria-pressed={item.live}><i></i></button>
{/snippet}

{#snippet last(item)}
  {#if item.last_run}
    <span class="last" title={relative(item.last_run.started, home.now)}><i class="dot {item.last_run.status}"></i>{relative(item.last_run.started, home.now)}</span>
  {/if}
{/snippet}

{#snippet tile(item)}
  {@const a = item.automation}
  {@const state = stateOf(item)}
  {@const g = gist(a)}
  <article class="card auto {state[0]}" title={item.summary}>
    <a class="open" href="#/automatismes/{a.id}" aria-label={t('automatismes.liste.ouvrir', { nom: a.name })}></a>
    <div class="head">
      <span class="ic"><Icon name={triggerIcon(a)} size={20} /></span>
      <h3>{a.name}</h3>
      {@render sw(item)}
      {@render more(item)}
    </div>
    <p class="when">{g.when}</p>
    <footer>
      {#each g.does as x, i (i)}<span class="chip"><Icon name={x.icon} size={13} />{x.text}</span>{/each}
      <span class="chip muted">{t('automatismes.liste.etapes', { count: g.steps })}</span>
      {#if state[0] !== 'live'}<span class="pill {state[0]}">{state[1]}</span>{/if}
      {#if AUTHOR[a.author]}<span class="pill">{AUTHOR[a.author]}</span>{/if}
      {#if item.protected?.length}<span class="pill lock"><Icon name="lock" size={12} />{item.protected.join(', ')}</span>{/if}
      {@render last(item)}
    </footer>
  </article>
{/snippet}

<svelte:window onclick={() => (menu = null)} />

{#if sub}
  <Editor id={sub} />
{:else}
  <div class="autos">
    <header class="top">
      <div>
        <h1 class="page-title">{t('automatismes.titre')}</h1>
        <p class="page-sub">{t('automatismes.liste.sous_titre')}</p>
      </div>
      <Personalize bind:editing lists={['auto-alertes', 'auto-scenarios']} />
    </header>

    {#if editing}<p class="hint">{t('maison.perso.aide_ordre')}</p>{/if}

    <form class="ask" onsubmit={(e) => (e.preventDefault(), draft())}>
      <Orb size={46} state={drafting ? 'thinking' : 'idle'} />
      <input bind:value={ask} placeholder={t('automatismes.liste.placeholder')} aria-label={t('automatismes.liste.decrire')} disabled={drafting} />
      <button disabled={!ask.trim() || drafting}>{drafting ? t('automatismes.moli_dessine') : t('automatismes.liste.creer')}</button>
    </form>

    <section class="ideas" class:compact={mine.length > 0} aria-label={t('automatismes.liste.idees')}>
      <button class="alert" onclick={() => newAlert()}>
        <span class="i"><Icon name="bell" size={20} /></span>
        <b>{t('automatismes.alerte.titre')}</b>
        <small>{t('automatismes.alerte.idee')}</small>
      </button>
      {#each IDEAS as [icon, id] (id)}
        <button onclick={() => draft(t('automatismes.idee.' + id + '.demande'))} disabled={drafting}>
          <span class="i"><Icon name={icon} size={20} /></span>
          <b>{t('automatismes.idee.' + id + '.titre')}</b>
          <small>{t('automatismes.idee.' + id + '.demande')}</small>
        </button>
      {/each}
      <a class="blank" href="#/automatismes/nouveau"><Icon name="plus" size={22} /><b>{t('automatismes.liste.partir_zero')}</b><small>{t('automatismes.liste.partir_zero_aide')}</small></a>
    </section>

    {#if error}<p class="muted">{error}</p>{/if}

    {#if list?.length}
      <h2 class="section">{t('automatismes.liste.section')} <span class="muted">{t('automatismes.liste.actifs', { count: list.filter((i) => i.live).length, total: mine.length })}</span></h2>
      {#if alertGroups.length}
        <h3 class="sub"><Icon name="bell" size={16} />{t('automatismes.liste.alertes')}</h3>
        {#snippet alertGroup(group)}
            {@const d = device(group.id)}
            <section class="card watch">
              <header>
                <h4>{nameOf(group.id)}{#if roomOf(d)}<small>{roomOf(d)}</small>{/if}</h4>
                <button class="add" onclick={() => newAlert(group.id)} aria-label={t('automatismes.liste.ajouter_alerte', { nom: nameOf(group.id) })}><Icon name="plus" size={16} /></button>
              </header>
              <ul>
                {#each group.items as { item, al } (item.automation.id)}
                  {@const st = stateOf(item)[0]}
                  <li class={st} title={item.summary}>
                    <a class="open" href="#/automatismes/{item.automation.id}" aria-label={t('automatismes.liste.ouvrir', { nom: item.automation.name })}></a>
                    <span class="ic"><Icon name={alertIcon(al)} size={17} /></span>
                    <span class="what">
                      <b>{al.label}</b>
                      <small>
                        {#each al.channels as c, i (c)}{#if i} · {/if}{channelName(c)}{/each}
                        {#if st !== 'live' && st !== 'off'} · {stateOf(item)[1]}{/if}
                      </small>
                    </span>
                    {@render last(item)}
                    {@render sw(item)}
                    {@render more(item)}
                  </li>
                {/each}
              </ul>
            </section>
        {/snippet}
        <div class="alerts">
          <Sortable list="auto-alertes" items={alertGroups} id={(g) => g.id} label={(g) => nameOf(g.id)} {editing} hideable={false} item={alertGroup} />
        </div>
      {/if}
      {#if scenarios.length}
        {#if alertGroups.length}<h3 class="sub"><Icon name="robot" size={16} />{t('automatismes.liste.scenarios')}</h3>{/if}
        <div class="grid">
          <Sortable list="auto-scenarios" items={scenarios} id={(x) => x.automation.id} label={(x) => x.automation.name} {editing} hideable={false} item={tile} />
        </div>
      {/if}
      {#if !mine.length}
        <p class="empty muted">{t('automatismes.liste.aucun_a_toi')}</p>
      {/if}
      {#if imported.length}
        <details class="imported">
          <summary>
            <Icon name="history" size={18} />
            <b>{t('automatismes.liste.importes_titre')}</b>
            <span class="muted">{t('automatismes.liste.importes', { count: imported.length })}</span>
          </summary>
          <p class="warn"><Icon name="alert" size={16} />{t('automatismes.liste.importes_avertissement')}</p>
          <div class="grid">
            {#each imported as item (item.automation.id)}{@render tile(item)}{/each}
          </div>
        </details>
      {/if}
    {:else if list}
      <p class="empty muted">{t('automatismes.liste.aucun')}</p>
    {/if}

    {#if recent.length}
      <!-- Folded: the list above is what matters; what ran is a click away. -->
      <details class="recent">
        <summary>
          <Icon name="history" size={18} />
          <b>{t('automatismes.liste.recent')}</b>
          <span class="muted">{t('automatismes.liste.dernier', { quand: relative(recent[0].started, home.now) })}</span>
          <span class="chev"><Icon name="chevron-down" size={18} /></span>
        </summary>
      <ul class="feed">
        {#each recent as r (r.id + (r.dry ? 'd' : ''))}
          <li>
            <i class="dot {r.status}"></i>
            <a href="#/automatismes/{r.automation}">{r.name}</a>
            <span class="muted">{r.dry ? t('automatismes.liste.essai_a_blanc') : r.why}</span>
            <small class="muted">{relative(r.started, home.now)}</small>
          </li>
        {/each}
      </ul>
      </details>
    {/if}

    {#if !abilities.telegram}
      <p class="muted hint"><Icon name="info" size={15} />{t('automatismes.liste.telegram_absent')}</p>
    {/if}
  </div>
{/if}

<style>
  .autos {
    display: grid;
    gap: 22px;
  }

  .ask {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 10px 10px 10px 14px;
    border-radius: 999px;
    background: var(--surface);
    box-shadow: var(--shadow-lift);
  }

  .ask input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: none;
    font: inherit;
    font-size: 19px;
    color: var(--ink);
    outline: none;
  }

  .ask button {
    border: 0;
    border-radius: 999px;
    padding: 14px 22px;
    background: var(--ink);
    color: var(--on-ink);
    font-weight: 700;
  }

  .ask button:disabled {
    opacity: 0.35;
  }

  .ideas {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
    gap: 12px;
  }

  .ideas button,
  .blank {
    display: grid;
    gap: 4px;
    align-content: start;
    padding: 16px;
    border: 0;
    border-radius: var(--r-md);
    background: var(--surface);
    box-shadow: var(--shadow);
    text-align: left;
    color: var(--ink);
    text-decoration: none;
    transition: transform 0.18s var(--ease);
  }

  .ideas button:hover,
  .blank:hover {
    transform: translateY(-2px);
  }

  .ideas .i {
    width: 38px;
    height: 38px;
    border-radius: 12px;
    display: grid;
    place-items: center;
    background: var(--warm-soft);
    color: var(--warm-ink);
    margin-bottom: 6px;
  }

  .ideas small,
  .blank small {
    font-size: 13px;
    color: var(--ink-3);
    line-height: 1.35;
  }

  .blank {
    background: none;
    box-shadow: inset 0 0 0 2px var(--line);
    color: var(--ink-2);
  }

  /* Once there are automations, ideas are a row of chips, not a wall. */
  .ideas.compact {
    display: flex;
    gap: 8px;
    overflow-x: auto;
    scrollbar-width: none;
    padding-bottom: 2px;
  }

  .ideas.compact button,
  .ideas.compact .blank {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 14px 8px 8px;
    border-radius: 999px;
    font-size: 14px;
    white-space: nowrap;
  }

  .ideas.compact .i {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    margin: 0;
  }

  .ideas.compact small {
    display: none;
  }

  .ideas.compact .blank {
    padding-left: 12px;
  }

  .ideas .alert .i {
    background: color-mix(in srgb, #b05fd8 14%, var(--surface));
    color: #b05fd8;
  }

  .sub {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: -6px;
    font-size: 12px;
    font-weight: 750;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--ink-3);
  }

  .alerts {
    --sort-cols: repeat(auto-fill, minmax(min(100%, 360px), 1fr));
    --sort-gap: 14px;
  }

  .alerts :global(.held > .card) {
    height: 100%;
  }

  .card.watch {
    padding: 14px 10px 8px;
  }

  .watch header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 6px 6px;
  }

  h4 {
    font-size: 16px;
    font-weight: 700;
    display: flex;
    align-items: baseline;
    gap: 8px;
  }

  h4 small {
    font-size: 13px;
    font-weight: 600;
    color: var(--ink-3);
  }

  .add {
    width: 32px;
    height: 32px;
    display: grid;
    place-items: center;
    border: 0;
    border-radius: 50%;
    background: var(--surface-2);
    color: var(--ink-2);
    cursor: pointer;
  }

  .watch ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
  }

  .watch li {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 6px;
    border-top: 1px solid var(--line);
    border-radius: 12px;
  }

  .watch li:first-child {
    border-top: 0;
  }

  .watch li:hover {
    background: var(--surface-2);
  }

  .watch li .ic {
    width: 32px;
    height: 32px;
    border-radius: 10px;
  }

  .watch li.off .what,
  .watch li.draft .what,
  .watch li.bad .what {
    opacity: 0.6;
  }

  .what {
    flex: 1;
    min-width: 0;
    display: grid;
  }

  .what b {
    font-size: 14.5px;
    font-weight: 650;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .what small {
    font-size: 12.5px;
    color: var(--ink-3);
  }

  .watch .switch {
    transform: scale(0.85);
  }

  .more {
    position: relative;
    z-index: 2;
  }

  .dots {
    width: 32px;
    height: 32px;
    display: grid;
    place-items: center;
    border: 0;
    border-radius: 50%;
    background: none;
    color: var(--ink-3);
    cursor: pointer;
  }

  .dots:hover,
  .dots[aria-expanded='true'] {
    background: var(--surface-3);
    color: var(--ink);
  }

  .menu {
    position: absolute;
    right: 0;
    top: calc(100% + 4px);
    z-index: 5;
    min-width: 170px;
    display: grid;
    padding: 6px;
    border-radius: 14px;
    background: var(--surface);
    box-shadow: var(--shadow-lift);
    border: 1px solid var(--line);
  }

  .menu a,
  .menu button {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 9px 10px;
    border: 0;
    border-radius: 10px;
    background: none;
    font: inherit;
    font-size: 14px;
    font-weight: 600;
    color: var(--ink);
    text-decoration: none;
    text-align: left;
    cursor: pointer;
  }

  .menu a:hover,
  .menu button:hover {
    background: var(--surface-2);
  }

  .menu .danger {
    color: var(--alert);
  }

  .when {
    color: var(--ink-2);
    font-size: 14px;
    line-height: 1.4;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 4px 9px;
    border-radius: 999px;
    font-size: 12px;
    font-weight: 650;
    background: var(--surface-2);
    color: var(--ink-2);
  }

  .chip.muted {
    background: none;
    color: var(--ink-3);
    padding-left: 2px;
  }

  .section {
    font-size: 15px;
    font-weight: 700;
    color: var(--ink-2);
    display: flex;
    gap: 10px;
    align-items: baseline;
  }

  .section span {
    font-weight: 500;
    font-size: 13px;
  }

  .grid {
    --sort-cols: repeat(auto-fill, minmax(min(100%, 300px), 1fr));
    --sort-gap: 14px;
  }

  .grid :global(.held > .card) {
    height: 100%;
  }

  .top {
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

  .card.auto {
    display: grid;
    gap: 8px;
    padding: 16px;
    transition: transform 0.18s var(--ease);
  }

  .auto:hover {
    transform: translateY(-2px);
  }

  .open {
    position: absolute;
    inset: 0;
    border-radius: inherit;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .ic {
    width: 40px;
    height: 40px;
    border-radius: 13px;
    display: grid;
    place-items: center;
    background: var(--surface-2);
    color: var(--ink-3);
  }

  .live .ic {
    background: color-mix(in srgb, #e9a23b 16%, var(--surface));
    color: #e9a23b;
  }

  h3 {
    flex: 1;
    font-size: 17px;
    font-weight: 700;
  }

  .switch {
    position: relative;
    z-index: 1;
    width: 46px;
    height: 28px;
    border-radius: 999px;
    border: 0;
    background: var(--surface-3);
  }

  .switch i {
    position: absolute;
    top: 4px;
    left: 4px;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--surface);
    box-shadow: 0 1px 3px rgb(0 0 0 / 20%);
    transition: transform 0.25s var(--ease);
  }

  .switch.on {
    background: var(--good);
  }

  .switch.on i {
    transform: translateX(18px);
  }

  .summary {
    color: var(--ink-2);
    font-size: 14.5px;
    line-height: 1.45;
  }

  footer {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    border-radius: 999px;
    font-size: 12px;
    font-weight: 700;
    background: var(--surface-2);
    color: var(--ink-3);
  }

  .pill.live {
    background: var(--good-soft);
    color: var(--good);
  }

  .pill.draft {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .pill.bad {
    background: var(--alert-soft);
    color: var(--alert);
  }

  .pill.lock {
    background: var(--cool-soft);
    color: var(--cool);
  }

  .last {
    margin-left: auto;
    flex: none;
    font-size: 12.5px;
    color: var(--ink-3);
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .dot {
    display: inline-block;
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--good);
  }

  .dot.failed {
    background: var(--alert);
  }

  .dot.cancelled {
    background: var(--ink-3);
  }

  .dot.running {
    background: var(--warm);
  }

  .imported {
    border-radius: var(--r-lg);
    background: color-mix(in srgb, var(--surface) 55%, transparent);
    padding: 14px 16px;
  }

  .imported summary {
    display: flex;
    align-items: center;
    gap: 10px;
    cursor: pointer;
    flex-wrap: wrap;
    list-style: none;
  }

  .imported summary span {
    font-size: 13px;
  }

  .imported[open] summary {
    margin-bottom: 14px;
  }

  /* As in the editor. */
  .warn {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13.5px;
    color: var(--cool);
    font-weight: 600;
    margin-bottom: 14px;
  }

  .recent {
    border-radius: var(--r-lg);
    background: color-mix(in srgb, var(--surface) 55%, transparent);
    padding: 14px 16px;
  }

  .recent summary {
    display: flex;
    align-items: center;
    gap: 10px;
    cursor: pointer;
    list-style: none;
    color: var(--ink-2);
  }

  .recent summary::-webkit-details-marker {
    display: none;
  }

  .recent summary span.muted {
    font-size: 13px;
  }

  .chev {
    margin-left: auto;
    display: grid;
    transition: transform 0.2s var(--ease);
  }

  .recent[open] .chev {
    transform: rotate(180deg);
  }

  .recent[open] summary {
    margin-bottom: 14px;
  }

  .feed {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 8px;
  }

  .feed li {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 14px;
  }

  .feed a {
    font-weight: 650;
    color: var(--ink);
    text-decoration: none;
  }

  .feed small {
    margin-left: auto;
  }

  .empty {
    padding: 20px 0;
  }

  .hint {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
  }

  @media (max-width: 560px) {
    .ask {
      border-radius: var(--r-lg);
      flex-wrap: wrap;
    }

    .ask input {
      font-size: 16px;
    }
  }
</style>

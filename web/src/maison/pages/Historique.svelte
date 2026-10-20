<script>
  import { onMount } from 'svelte';
  import { hub, device, roomOf, clock, longDate, num } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from '../ui/Icon.svelte';

  /** What was ordered or changed in the house, newest first and by day: who
   *  did it (the family, Moli, an automation, a phone), to what, and how it
   *  went. The hub keeps the last 500. */

  const PAGE = 60;
  const ORIGIN = ['ui', 'household', 'api', 'mcp', 'cli', 'system', 'assistant', 'automation'];
  const CHANGE = ['created', 'updated', 'approved', 'switched_off', 'restored', 'deleted'];
  const VERDICT = ['approved', 'denied', 'expired'];
  const word = (table, area, id) => (table.includes(id) ? t(`systeme.journal.${area}.${id}`) : id);

  let past = $state([]);
  onMount(async () => {
    try {
      const res = await fetch('/api/journal?limit=500');
      if (res.ok) past = await res.json();
    } catch {
      /* the live ones below still show */
    }
  });

  const entries = $derived.by(() => {
    const byId = new Map([...past, ...hub.journal].map((e) => [e.id, e]));
    return [...byId.values()].sort((a, b) => b.id - a.id);
  });

  /** The device's name; its id when it is gone. */
  const named = (id) => device(id)?.label?.name || device(id)?.native_name || id;
  const splitPoint = (p) => [p.slice(0, p.indexOf('/')), p.slice(p.indexOf('/') + 1)];

  function unit(v, p) {
    if (typeof v === 'number') return `${num(v, Number.isInteger(v) ? 0 : 1)}${p?.unit ? ` ${p.unit}` : ''}`;
    return String(v);
  }

  /** One entry as a sentence: what it touched, what happened, an icon, a type. */
  function describe(e) {
    const a = e.action;
    if (a.kind === 'command') {
      const [id, key] = splitPoint(a.point);
      const dev = hub.devices[id];
      const point = dev?.points.find((p) => p.key === key);
      const on = point?.kind?.type === 'binary';
      const lamp = ['on', 'power', 'state'].includes(key) || /^switch/.test(key);
      return {
        type: 'commande',
        icon: key === 'target_temperature' ? 'air-conditioner' : on ? (lamp ? 'light' : 'power') : ['playing', 'volume'].includes(key) ? 'speaker' : /control|position/.test(key) ? 'home-roof' : key === 'mode' ? 'shield' : 'tune',
        target: named(id),
        room: dev ? roomOf(dev) : null,
        what: on ? (a.value ? t('maison.historique.allume') : t('maison.historique.eteint')) : `${point?.label ?? key} → ${unit(a.value, point)}`,
        on: on ? !!a.value : null,
      };
    }
    if (a.kind === 'automation') {
      return { type: 'auto', icon: 'robot', target: t('systeme.journal.automatisme', { name: a.name }), what: word(CHANGE, 'changement', a.change) };
    }
    if (a.kind === 'approval') {
      const [id] = splitPoint(a.point);
      return {
        type: 'demande',
        icon: 'lock',
        target: named(id),
        what: t('systeme.journal.demande', { request: a.request, verdict: word(VERDICT, 'verdict', a.decision) }),
      };
    }
    if (a.kind === 'address') {
      return { type: 'reglage', icon: 'info', target: a.device ? named(a.device) : a.instance, what: t('systeme.journal.adresse', { from: a.from, to: a.to }) };
    }
    const parts = [a.label?.name, a.label?.room].filter(Boolean).join(' · ');
    return { type: 'reglage', icon: 'pencil', target: named(a.device), what: t('systeme.journal.etiquette', { value: parts || t('systeme.journal.effacee') }) };
  }

  /** « Famille », « Moli », « HomeKit »… the actor without the address. */
  function who(e) {
    const actor = e.actor?.replace(/\s*\(.*\)\s*$/, '').trim();
    const origin = word(ORIGIN, 'origine', e.origin);
    if (!actor || origin.toLowerCase().includes(actor.toLowerCase())) return origin;
    return e.origin === 'api' ? (actor === 'homekit' ? 'HomeKit' : actor.charAt(0).toUpperCase() + actor.slice(1)) : `${origin} · ${actor}`;
  }

  const FILTERS = ['all', 'commande', 'auto', 'demande', 'reglage', 'probleme'];
  let filter = $state('all');
  let query = $state('');
  let shown = $state(PAGE);

  const norm = (s) => String(s ?? '').normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();
  const rows = $derived(entries.map((e) => ({ e, d: describe(e), who: who(e) })));
  const matching = $derived.by(() => {
    const q = norm(query);
    return rows.filter(
      (r) =>
        (filter === 'all' || (filter === 'probleme' ? r.e.outcome.result !== 'ok' : r.d.type === filter)) &&
        (!q || norm(`${r.d.target} ${r.d.what} ${r.who} ${r.d.room ?? ''}`).includes(q)),
    );
  });
  const count = (f) => rows.filter((r) => (f === 'all' ? true : f === 'probleme' ? r.e.outcome.result !== 'ok' : r.d.type === f)).length;

  const dayOf = (ts) => new Date(ts).toDateString();
  function dayName(ts) {
    const today = new Date(hub.now ?? Date.now());
    if (dayOf(ts) === today.toDateString()) return t('maison.historique.aujourdhui');
    if (dayOf(ts) === new Date(today.getTime() - 86_400_000).toDateString()) return t('maison.historique.hier');
    return longDate(ts);
  }
  const days = $derived.by(() => {
    const out = [];
    for (const r of matching.slice(0, shown)) {
      const key = dayOf(r.e.ts);
      if (out.at(-1)?.key !== key) out.push({ key, name: dayName(r.e.ts), rows: [] });
      out.at(-1).rows.push(r);
    }
    return out;
  });
</script>

<div class="historique">
  <header>
    <h1 class="page-title">{t('maison.historique.titre')}</h1>
    <p class="page-sub">{t('maison.historique.sous_titre')}</p>
  </header>

  <div class="bar">
    <label class="search">
      <Icon name="search" size={16} />
      <input bind:value={query} oninput={() => (shown = PAGE)} placeholder={t('maison.historique.chercher')} aria-label={t('maison.historique.chercher')} />
    </label>
    <div class="chips" role="group" aria-label={t('maison.historique.filtrer')}>
      {#each FILTERS as f (f)}
        {#if f === 'all' || count(f)}
          <button class:on={filter === f} class:bad={f === 'probleme'} aria-pressed={filter === f} onclick={() => ((filter = f), (shown = PAGE))}>
            {t(`maison.historique.f_${f}`)}{#if f === 'probleme'} <small>{count(f)}</small>{/if}
          </button>
        {/if}
      {/each}
    </div>
  </div>

  {#each days as day (day.key)}
    <section class="day">
      <h2>{day.name}</h2>
      <ul>
        {#each day.rows as { e, d, who: by } (e.id)}
          <li class:err={e.outcome.result === 'err'} class:held={e.outcome.result === 'pending'}>
            <time datetime={new Date(e.ts).toISOString()}>{clock(e.ts)}</time>
            <span class="glyph" class:lit={d.on === true}><Icon name={d.icon} size={18} /></span>
            <span class="what">
              <span class="line"><b>{d.target}</b> <span class="verb" class:lit={d.on === true}>{d.what}</span>{#if d.room}<span class="room">{d.room}</span>{/if}</span>
              <small>{by}{#if e.outcome.result === 'err'} · <span class="bad">{t('maison.historique.echec', { error: e.outcome.error })}</span>{/if}{#if e.outcome.result === 'pending'} · <span class="wait">{t('maison.historique.retenu')}</span>{/if}</small>
            </span>
          </li>
        {/each}
      </ul>
    </section>
  {:else}
    <p class="empty">{entries.length ? t('maison.historique.vide_filtre') : t('maison.historique.vide')}</p>
  {/each}

  {#if matching.length > shown}
    <button class="more" onclick={() => (shown += PAGE)}>{t('maison.historique.plus', { count: matching.length - shown })}</button>
  {/if}
</div>

<style>
  .historique {
    display: grid;
    gap: 20px;
    max-width: 860px;
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
    max-width: 300px;
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

  .chips button.bad:not(.on) {
    color: var(--alert);
  }

  .day h2 {
    position: sticky;
    top: 0;
    z-index: 1;
    margin: 0 0 6px;
    padding: 8px 0;
    font-size: 13px;
    font-weight: 750;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ink-2);
    background: var(--bg, transparent);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    border-radius: 18px;
    background: var(--surface);
    overflow: hidden;
  }

  li {
    display: grid;
    grid-template-columns: 46px 36px minmax(0, 1fr);
    align-items: center;
    gap: 12px;
    padding: 10px 16px;
    border-top: 1px solid var(--line);
  }

  li:first-child {
    border-top: 0;
  }

  li.err {
    background: color-mix(in srgb, var(--alert) 8%, transparent);
  }

  time {
    font-size: 13px;
    font-variant-numeric: tabular-nums;
    color: var(--ink-3);
  }

  .glyph {
    width: 36px;
    height: 36px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: var(--surface-2);
    color: var(--ink-3);
  }

  .glyph.lit {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .what {
    display: grid;
    min-width: 0;
  }

  .line {
    overflow-wrap: anywhere;
  }

  .verb {
    color: var(--ink-2);
  }

  .verb.lit {
    color: var(--warm-ink);
    font-weight: 650;
  }

  .room {
    margin-left: 8px;
    padding: 1px 8px;
    border-radius: 999px;
    font-size: 12px;
    background: var(--surface-2);
    color: var(--ink-3);
  }

  small {
    font-size: 12.5px;
    color: var(--ink-3);
  }

  .bad {
    color: var(--alert);
  }

  .wait {
    color: var(--warm-ink);
  }

  .empty {
    color: var(--ink-3);
  }

  .more {
    justify-self: center;
    border: 0;
    border-radius: 999px;
    padding: 9px 20px;
    font: inherit;
    font-weight: 650;
    background: var(--surface-2);
    color: var(--ink-2);
    cursor: pointer;
  }

  @media (max-width: 560px) {
    li {
      grid-template-columns: 36px minmax(0, 1fr);
    }

    time {
      display: none;
    }
  }
</style>

<script>
  import { onMount } from 'svelte';
  import { hub, home, note } from '../lib/home.svelte.js';
  import { setLabel } from '../../lib/hub.svelte.js';
  import { t, locale } from '../../lib/i18n.svelte.js';
  import { CATEGORIES, categoryOf } from '../lib/filters.js';
  import Icon from './Icon.svelte';

  /** What the voice assistants see, in one table: per device, the name they
   *  hear (shared by HomeKit and Alexa) and whether each one shows it.
   *  What leaves the house is a person's choice (code asked). */
  let homekit = $state(null);
  let alexa = $state(null);
  let filter = $state('');
  let busy = $state(false);

  /** This viewer's way of reading the table, kept in the browser (may be absent). */
  const VIEW_KEY = 'moli-voice-view';
  const view = $state({ sort: 'room', scope: 'homekit', folds: { on: true, off: false } });
  try {
    const kept = JSON.parse(localStorage.getItem(VIEW_KEY) ?? '{}');
    if (['room', 'name', 'type'].includes(kept.sort)) view.sort = kept.sort;
    if (['homekit', 'alexa'].includes(kept.scope)) view.scope = kept.scope;
    if (kept.folds) view.folds = { on: kept.folds.on !== false, off: kept.folds.off === true };
  } catch {
    // Private window: the choice lasts the visit.
  }
  function keepView() {
    try {
      localStorage.setItem(VIEW_KEY, JSON.stringify(view));
    } catch {
      // Same.
    }
  }
  function setView(key, value) {
    view[key] = value;
    keepView();
  }

  async function load() {
    const get = async (path) => {
      try {
        const res = await fetch(path);
        return res.ok ? await res.json() : null;
      } catch {
        return null;
      }
    };
    [homekit, alexa] = await Promise.all([get('/api/homekit'), get('/api/integrations/alexa')]);
  }
  onMount(load);

  const hk = $derived(homekit?.enabled ? homekit : null);
  const ax = $derived(alexa?.enabled ? alexa : null);

  /** One row per device either assistant has a place for; a HomeKit camera
   *  (its live image) is a row of its own. */
  const rows = $derived.by(() => {
    const byId = new Map();
    const row = (id) => {
      if (!byId.has(id)) {
        const camera = id.endsWith('#camera');
        const device = hub.devices[camera ? id.slice(0, -7) : id];
        byId.set(id, { id, camera, device, homekit: null, alexa: null });
      }
      return byId.get(id);
    };
    for (const d of hk?.devices ?? []) Object.assign(row(d.id), { homekit: d, fallback: d.name, room: d.room });
    for (const d of ax?.devices ?? []) row(d.id).alexa = d;
    const list = [...byId.values()].map((r) => ({
      ...r,
      name: r.device?.label?.name || r.device?.native_name || r.fallback || r.id,
      room: r.device?.label?.room || r.device?.native_room || r.room || '',
    }));
    list.sort((a, b) => byName(a.name, b.name) || (a.camera ? 1 : 0) - (b.camera ? 1 : 0));
    return list;
  });

  const byName = (a, b) => a.localeCompare(b, locale(), { numeric: true, sensitivity: 'base' });
  const norm = (s) => String(s ?? '').normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();

  const searching = $derived(filter.trim() !== '');
  const shown = $derived.by(() => {
    const q = norm(filter.trim());
    if (!q) return rows;
    return rows.filter((r) => norm(`${r.name} ${r.device?.label?.voice ?? ''} ${r.room} ${r.id}`).includes(q));
  });

  /** The assistant the two folds speak for: the only one there is, else the chosen one. */
  const scope = $derived(hk && ax ? view.scope : ax ? 'alexa' : 'homekit');
  const scopeName = $derived(scope === 'alexa' ? 'Alexa' : 'HomeKit');
  const isShown = (r) => {
    const entry = r[scope];
    return Boolean(entry?.exposed && !entry.protected);
  };

  /** Where a row goes under the chosen sort, as `[key, title]`. */
  function place(r) {
    if (view.sort === 'room') return [r.room, r.room || t('systeme.vocal.sans_piece')];
    if (view.sort === 'type') {
      const id = r.camera ? 'cameras' : r.device ? categoryOf(r.device) : 'capteurs';
      const category = CATEGORIES.find((c) => c.id === id);
      return [t(category.label), t(category.label)];
    }
    return ['', ''];
  }

  /** Under one fold: rows by room or by type in the alphabet, nameless last. */
  function grouped(list) {
    const map = new Map();
    for (const r of list) {
      const [key, title] = place(r);
      if (!map.has(key)) map.set(key, { key, title, rows: [] });
      map.get(key).rows.push(r);
    }
    return [...map.values()].sort((a, b) => (!a.key) - (!b.key) || byName(a.key, b.key));
  }

  const folds = $derived([
    { id: 'on', title: t('systeme.vocal.visibles', { name: scopeName }), rows: shown.filter(isShown) },
    { id: 'off', title: t('systeme.vocal.masques', { name: scopeName }), rows: shown.filter((r) => !isShown(r)) },
  ]);

  /** A person's gesture: the code first, then the same request again. */
  async function write(path, body, label, again) {
    busy = true;
    try {
      const res = await fetch(path, {
        method: 'PUT',
        headers: { 'content-type': 'application/json', 'x-moli-origin': 'ui' },
        body: JSON.stringify(body),
      });
      if (res.status === 403) {
        home.held = { label, reason: t('systeme.homekit.code_demande'), custom: again };
        return false;
      }
      const reply = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(reply.error ?? res.statusText);
      await load();
      // The HomeKit bridge rebuilds its database a moment after a change.
      setTimeout(load, 900);
      return true;
    } catch (e) {
      note(t('systeme.vocal.erreur', { message: e.message }), 'error');
      return false;
    } finally {
      busy = false;
    }
  }

  const setHomeKit = (r, exposed) =>
    write(`/api/homekit/devices/${encodeURIComponent(r.id)}`, { exposed }, r.name, () => setHomeKit(r, exposed));

  async function setAlexa(r, exposed) {
    const done = await write(`/api/integrations/alexa/${encodeURIComponent(r.id)}`, { exposed }, r.name, () => setAlexa(r, exposed));
    if (done) note(t('systeme.vocal.decouvrir'), 'info');
  }

  async function setVoice(r, input) {
    const voice = input.value.trim();
    if (voice === (r.device?.label?.voice ?? '')) return;
    await setLabel(r.id, { voice });
    setTimeout(load, 900);
    if (r.alexa?.exposed) note(t('systeme.vocal.decouvrir'), 'info');
  }

  const ago = (ms) => {
    const min = Math.round((Date.now() - ms) / 60000);
    if (min < 1) return t('systeme.vocal.a_l_instant');
    if (min < 60) return t('systeme.vocal.il_y_a_min', { count: min });
    return new Date(ms).toLocaleString(locale(), { dateStyle: 'short', timeStyle: 'short' });
  };
</script>

{#if homekit && alexa}
  <section class="voice">
    <div class="head">
      <span class="ico"><Icon name="mic" size={18} /></span>
      <div class="text">
        <b>{t('systeme.vocal.titre')}</b>
        <span class="muted">{t('systeme.vocal.aide')}</span>
      </div>
    </div>

    <ul class="status">
      <li>
        <b>HomeKit</b>
        <span class="muted">
          {hk ? t('systeme.homekit.resume', { devices: hk.status.exposed, count: hk.status.controllers }) : t('systeme.vocal.homekit_absent')}
        </span>
      </li>
      <li>
        <b>Alexa</b>
        <span class="muted">
          {#if !ax}
            {t('systeme.vocal.alexa_absent')}
          {:else if !ax.token}
            {t('systeme.vocal.alexa_sans_jeton')}
          {:else}
            {t('systeme.vocal.alexa_resume', { count: ax.devices.filter((d) => d.exposed).length })}
            · {ax.last ? t('systeme.vocal.derniere', { when: ago(ax.last) }) : t('systeme.vocal.jamais')}
          {/if}
        </span>
      </li>
    </ul>

    {#if rows.length}
      <div class="tools">
        <input class="filter" type="search" placeholder={t('systeme.homekit.filtrer')} aria-label={t('systeme.homekit.filtrer')} bind:value={filter} spellcheck="false" autocomplete="off" />
        <div class="chips" role="radiogroup" aria-label={t('systeme.vocal.trier')}>
          <span class="muted">{t('systeme.vocal.trier')}</span>
          {#each ['room', 'name', 'type'] as s (s)}
            <button role="radio" aria-checked={view.sort === s} class:on={view.sort === s} onclick={() => setView('sort', s)}>{t(`systeme.vocal.par_${s}`)}</button>
          {/each}
        </div>
        {#if hk && ax}
          <div class="chips" role="radiogroup" aria-label={t('systeme.vocal.assistant')}>
            <span class="muted">{t('systeme.vocal.assistant')}</span>
            {#each ['homekit', 'alexa'] as s (s)}
              <button role="radio" aria-checked={view.scope === s} class:on={view.scope === s} onclick={() => setView('scope', s)}>{s === 'alexa' ? 'Alexa' : 'HomeKit'}</button>
            {/each}
          </div>
        {/if}
      </div>

      {#each folds as fold (fold.id)}
        <details
          class="fold"
          open={searching || view.folds[fold.id]}
          ontoggle={(e) => {
            if (searching || view.folds[fold.id] === e.currentTarget.open) return;
            view.folds[fold.id] = e.currentTarget.open;
            keepView();
          }}
        >
          <summary>{fold.title} <small>{fold.rows.length}</small></summary>
          {#if fold.rows.length}
            <div class="table" role="table" aria-label={fold.title}>
              <div class="tr th" role="row">
                <span role="columnheader">{t('systeme.vocal.appareil')}</span>
                <span role="columnheader">{t('systeme.vocal.nom_vocal')}</span>
                <span role="columnheader" class="col">HomeKit</span>
                <span role="columnheader" class="col">Alexa</span>
              </div>
              {#each grouped(fold.rows) as group (group.key)}
                {#if group.title}<div class="group-title" role="rowgroup">{group.title} <small>{group.rows.length}</small></div>{/if}
                {#each group.rows as r (r.id)}
                  <div class="tr" role="row">
                    <span class="who" role="cell">
                      <b>{r.name}{#if r.camera}<span class="muted"> · {t('systeme.homekit.type.camera')}</span>{/if}</b>
                      {#if r.room && view.sort !== 'room'}<small class="muted">{r.room}</small>{/if}
                      {#if r.alexa?.names.length > 1}<small class="muted">{t('systeme.vocal.alexa_noms', { names: r.alexa.names.join(', ') })}</small>{/if}
                      {#if r.homekit?.absent}<small class="chip warm">{t('systeme.homekit.absent')}</small>{/if}
                    </span>
                    <span role="cell">
                      {#if r.device && !r.camera}
                        <input
                          class="voice-name"
                          type="text"
                          maxlength="60"
                          value={r.device.label?.voice ?? ''}
                          placeholder={r.name}
                          aria-label={t('systeme.vocal.nom_vocal_de', { name: r.name })}
                          onchange={(e) => setVoice(r, e.currentTarget)}
                          onkeydown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
                        />
                      {/if}
                    </span>
                    <label class="col" role="cell" title={r.homekit?.protected ? t('systeme.homekit.protegee_aide') : 'HomeKit'}>
                      {#if r.homekit}
                        <span class="phone-only">HomeKit</span>
                        <input type="checkbox" checked={r.homekit.exposed && !r.homekit.protected} disabled={busy || r.homekit.protected} onchange={(e) => setHomeKit(r, e.currentTarget.checked)} />
                      {:else}
                        <span class="none" aria-label={t('systeme.vocal.sans_objet')}>—</span>
                      {/if}
                    </label>
                    <label class="col" role="cell" title="Alexa">
                      {#if r.alexa}
                        <span class="phone-only">Alexa</span>
                        <input type="checkbox" checked={r.alexa.exposed} disabled={busy} onchange={(e) => setAlexa(r, e.currentTarget.checked)} />
                      {:else}
                        <span class="none" aria-label={t('systeme.vocal.sans_objet')}>—</span>
                      {/if}
                    </label>
                  </div>
                {/each}
              {/each}
            </div>
          {:else}
            <p class="muted empty">{searching ? t('systeme.homekit.aucun') : t('systeme.vocal.vide')}</p>
          {/if}
        </details>
      {/each}
      <small class="muted">{t('systeme.vocal.pied')}</small>
    {/if}
  </section>
{/if}

<style>
  .voice {
    display: grid;
    gap: 14px;
    padding: 16px 18px;
    border-radius: var(--r-md);
    background: var(--surface-2);
  }

  .head {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .ico {
    width: 36px;
    height: 36px;
    flex: none;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--surface);
    color: var(--warm-ink);
  }

  .text {
    display: grid;
    gap: 2px;
  }

  .text .muted {
    font-size: 13px;
  }

  .status {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 6px;
    font-size: 14px;
  }

  .status li {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
  }

  .status b {
    min-width: 70px;
  }

  .filter,
  .voice-name {
    font: inherit;
    font-size: 14px;
    padding: 8px 12px;
    border-radius: var(--r-sm);
    border: 1px solid var(--line);
    background: var(--surface);
    color: var(--ink);
    width: 100%;
    box-sizing: border-box;
  }

  .tools {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px 16px;
  }

  .tools .filter {
    flex: 1 1 220px;
    width: auto;
    min-width: 0;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    font-size: 13px;
  }

  .chips button {
    border: 0;
    border-radius: 999px;
    padding: 6px 12px;
    font: inherit;
    font-weight: 650;
    background: var(--surface);
    color: var(--ink-2);
    cursor: pointer;
  }

  .chips button.on {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .fold {
    border-radius: var(--r-sm);
    background: var(--surface);
    padding: 2px 10px;
  }

  .fold summary {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 2px;
    font-weight: 650;
    cursor: pointer;
    list-style: none;
  }

  .fold summary::-webkit-details-marker {
    display: none;
  }

  .fold summary::before {
    content: '›';
    width: 12px;
    font-size: 18px;
    line-height: 1;
    color: var(--ink-2);
    transition: transform 0.15s;
  }

  .fold[open] summary::before {
    transform: rotate(90deg);
  }

  .fold summary small {
    color: var(--ink-2);
  }

  .group-title {
    margin-top: 10px;
    padding: 4px 8px;
    font-size: 13px;
    font-weight: 700;
    color: var(--warm-ink);
    border-bottom: 1px solid var(--line);
  }

  .group-title small {
    font-weight: 600;
    color: var(--ink-2);
  }

  .empty {
    margin: 0 0 10px;
    font-size: 14px;
  }

  .table {
    display: grid;
    gap: 2px;
    padding-bottom: 8px;
  }

  .fold .tr:not(.th):hover {
    background: var(--surface-2);
  }

  .fold .voice-name {
    background: var(--surface-2);
  }

  .tr {
    display: grid;
    grid-template-columns: minmax(0, 1.2fr) minmax(0, 1fr) 72px 72px;
    gap: 12px;
    align-items: center;
    padding: 6px 8px;
    border-radius: var(--r-sm);
  }

  .tr:not(.th):hover {
    background: var(--surface);
  }

  .th {
    font-size: 12px;
    font-weight: 650;
    color: var(--ink-2);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .who {
    display: grid;
    gap: 2px;
    min-width: 0;
  }

  .who b {
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  .col {
    display: flex;
    justify-content: center;
    align-items: center;
    gap: 6px;
  }

  .col input {
    width: 18px;
    height: 18px;
    accent-color: var(--warm);
    cursor: pointer;
  }

  .none {
    color: var(--ink-2);
  }

  .chip {
    justify-self: start;
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
  }

  .chip.warm {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .phone-only {
    display: none;
  }

  small {
    font-size: 12px;
  }

  /* A phone: each device on its own small card, the two choices side by side. */
  @media (max-width: 640px) {
    .th {
      display: none;
    }

    .tr {
      grid-template-columns: 1fr 1fr;
      background: var(--surface-2);
      padding: 10px 12px;
      margin-bottom: 6px;
    }

    .who,
    .tr > span[role='cell']:nth-child(2) {
      grid-column: 1 / -1;
    }

    .col {
      justify-content: flex-start;
    }

    .phone-only {
      display: inline;
      font-size: 13px;
    }
  }
</style>

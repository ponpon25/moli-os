<script>
  import { onMount } from 'svelte';
  import { hub, home, note } from '../lib/home.svelte.js';
  import { setLabel } from '../../lib/hub.svelte.js';
  import { t, locale } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** What the voice assistants see, in one table: per device, the name they
   *  hear (shared by HomeKit and Alexa) and whether each one shows it.
   *  What leaves the house is a person's choice (code asked). */
  let homekit = $state(null);
  let alexa = $state(null);
  let filter = $state('');
  let busy = $state(false);

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
    list.sort((a, b) => a.room.localeCompare(b.room, locale()) || a.name.localeCompare(b.name, locale()) || (a.camera ? 1 : 0) - (b.camera ? 1 : 0));
    return list;
  });

  const shown = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return rows;
    return rows.filter((r) => `${r.name} ${r.device?.label?.voice ?? ''} ${r.room} ${r.id}`.toLowerCase().includes(q));
  });

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
      <input class="filter" type="search" placeholder={t('systeme.homekit.filtrer')} bind:value={filter} spellcheck="false" autocomplete="off" />

      <div class="table" role="table" aria-label={t('systeme.vocal.titre')}>
        <div class="tr th" role="row">
          <span role="columnheader">{t('systeme.vocal.appareil')}</span>
          <span role="columnheader">{t('systeme.vocal.nom_vocal')}</span>
          <span role="columnheader" class="col">HomeKit</span>
          <span role="columnheader" class="col">Alexa</span>
        </div>
        {#each shown as r (r.id)}
          <div class="tr" role="row">
            <span class="who" role="cell">
              <b>{r.name}{#if r.camera}<span class="muted"> · {t('systeme.homekit.type.camera')}</span>{/if}</b>
              {#if r.room}<small class="muted">{r.room}</small>{/if}
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
        {:else}
          <p class="muted">{t('systeme.homekit.aucun')}</p>
        {/each}
      </div>
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

  .table {
    display: grid;
    gap: 2px;
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
      background: var(--surface);
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

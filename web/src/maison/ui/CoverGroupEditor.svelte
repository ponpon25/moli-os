<script>
  import { onMount } from 'svelte';
  import { hub, device, deviceKind, isCoverGroup, nameOf, note, protectedRoom, reachable, roomOf } from '../lib/home.svelte.js';
  import { showOnDashboard } from '../lib/layout.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** Makes or changes a group of shutters: its name and its shutters, a
   *  shutter being free to be in other groups as well. `id`: the group's
   *  device; none, a new group. */
  let { id = null, onclose } = $props();

  let dialog;
  onMount(() => dialog.showModal());

  const group = $derived(device(id));
  // What the group is when the dialog opens; afterwards, it is the family's.
  const start = device(id);
  let name = $state(start ? nameOf(start.id) : '');
  let chosen = $state(new Set(start?.members ?? []));
  let busy = $state(false);

  // ---- switches: a real switch's presses open, close or stop the group ----
  const SLOTS = ['open', 'close', 'stop'];
  const SLOT_WORD = { open: 'ouvrir', close: 'fermer', stop: 'arreter' };
  /** `[{ device, open, close, stop }]`, as the server keeps them. */
  let switches = $state([]);
  /** Read once: a family's change while the dialog is open is not lost. */
  let switchesRead = $state(!id);
  /** `{ device, slot, since }` while waiting for a press. */
  let learning = $state(null);

  onMount(async () => {
    if (!id) return;
    try {
      const res = await fetch('/api/cover-groups');
      const answer = await res.json();
      const mine = (answer.groups ?? []).find((g) => g.device === id);
      switches = (mine?.switches ?? []).map((s) => ({ device: s.device, open: s.open ?? null, close: s.close ?? null, stop: s.stop ?? null }));
      switchesRead = true;
    } catch (err) {
      note(t('maison.groupes_stores.echec', { error: err.message }), 'error');
    }
  });

  /** Devices whose presses arrive on `action` (Zigbee remotes). */
  const remotes = $derived(
    Object.values(hub.devices)
      .filter((d) => d.points.some((p) => p.key === 'action'))
      .map((d) => ({ id: d.id, name: nameOf(d.id), room: roomOf(d) }))
      .sort((a, b) => collator.compare(a.name, b.name)),
  );
  const addable = $derived(remotes.filter((r) => !switches.some((s) => s.device === r.id)));

  function addSwitch(deviceId) {
    if (!deviceId) return;
    switches = [...switches, { device: deviceId, open: null, close: null, stop: null }];
  }

  function removeSwitch(deviceId) {
    switches = switches.filter((s) => s.device !== deviceId);
    if (learning?.device === deviceId) learning = null;
  }

  function setPress(deviceId, slot, press) {
    switches = switches.map((s) => {
      if (s.device !== deviceId) return s;
      const next = { ...s, [slot]: press };
      // One button, one meaning: taken off the slot that had it.
      for (const other of SLOTS) {
        if (other !== slot && press && next[other] === press) {
          next[other] = null;
          note(t('maison.groupes_stores.deja_prise', { what: t(`maison.groupes_stores.${SLOT_WORD[other]}`) }));
        }
      }
      return next;
    });
  }

  function learn(deviceId, slot) {
    const now = device(deviceId)?.state?.action?.ts ?? 0;
    learning = learning?.device === deviceId && learning.slot === slot ? null : { device: deviceId, slot, since: now };
  }

  // The first press after « Apprendre » is the one.
  $effect(() => {
    if (!learning) return;
    const seen = device(learning.device)?.state?.action;
    if (seen && seen.ts > learning.since && typeof seen.value === 'string' && seen.value !== '') {
      const { device: d, slot } = learning;
      learning = null;
      setPress(d, slot, seen.value);
    }
  });

  /** `on_top` → « on top »: readable enough beside the learning. */
  const pressWord = (p) => (p ? p.replaceAll('_', ' ') : t('maison.groupes_stores.aucune_touche'));

  const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
  /** Every shutter, by room; a member whose driver is away stays listed. */
  const shutters = $derived.by(() => {
    const known = Object.values(hub.devices).filter((d) => deviceKind(d) === 'cover' && !isCoverGroup(d));
    const away = [...chosen].filter((m) => !device(m)).map((m) => ({ id: m, away: true }));
    return [...known, ...away]
      .map((d) => ({ id: d.id, away: !!d.away, room: d.away ? null : roomOf(d), name: d.away ? d.id : nameOf(d.id) }))
      .sort((a, b) => (!a.room - !b.room) || collator.compare(a.room ?? '', b.room ?? '') || collator.compare(a.name, b.name));
  });
  const guarded = $derived(shutters.some((s) => chosen.has(s.id) && s.room && protectedRoom(s.room)));
  const ready = $derived(name.trim() !== '' && chosen.size > 0 && !busy);

  function pick(member, yes) {
    const next = new Set(chosen);
    if (yes) next.add(member);
    else next.delete(member);
    chosen = next;
  }

  async function send(method, path, body) {
    busy = true;
    try {
      const res = await fetch(path, {
        method,
        headers: { 'content-type': 'application/json', 'x-moli-origin': 'ui' },
        body: body ? JSON.stringify(body) : undefined,
      });
      const answer = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(answer.error ?? res.statusText);
      return answer;
    } catch (err) {
      note(t('maison.groupes_stores.echec', { error: err.message }), 'error');
      return null;
    } finally {
      busy = false;
    }
  }

  const native = $derived(group?.id.split(':')[1]);

  async function save(e) {
    e.preventDefault();
    if (!ready) return;
    const body = { name, members: [...chosen] };
    if (switchesRead) body.switches = switches.filter((s) => SLOTS.some((k) => s[k]));
    const done = id ? await send('PUT', `/api/cover-groups/${encodeURIComponent(native)}`, body) : await send('POST', '/api/cover-groups', body);
    if (!done) return;
    if (!id) showOnDashboard(done.device, true);
    dialog.close();
  }

  async function remove() {
    if (!confirm(t('maison.groupes_stores.supprimer_confirme', { name: nameOf(id) }))) return;
    if (await send('DELETE', `/api/cover-groups/${encodeURIComponent(native)}`)) dialog.close();
  }
</script>

<dialog bind:this={dialog} {onclose} onclick={(e) => e.target === dialog && dialog.close()} aria-label={t(id ? 'maison.groupes_stores.modifier' : 'maison.groupes_stores.creer')}>
  <form onsubmit={save}>
    <header>
      <h2>{t(id ? 'maison.groupes_stores.modifier' : 'maison.groupes_stores.creer')}</h2>
      <button type="button" class="close" onclick={() => dialog.close()} aria-label={t('commun.fermer')}><Icon name="close" size={22} /></button>
    </header>

    <label class="name">
      <span>{t('maison.groupes_stores.nom')}</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input bind:value={name} maxlength="60" placeholder={t('maison.groupes_stores.nom_exemple')} autofocus={!id} />
    </label>

    <fieldset>
      <legend>{t('maison.groupes_stores.membres', { count: chosen.size })}</legend>
      {#each shutters as s (s.id)}
        <label class="member">
          <input type="checkbox" checked={chosen.has(s.id)} onchange={(e) => pick(s.id, e.currentTarget.checked)} />
          <span class="who">
            <b>{s.name}</b>
            {#if s.away || !reachable(s.id)}
              <small>{t('commun.volet.injoignable')}</small>
            {:else if s.room}
              <small>{s.room}</small>
            {/if}
          </span>
        </label>
      {:else}
        <p class="help">{t('maison.groupes_stores.aucun_store')}</p>
      {/each}
    </fieldset>

    <fieldset class="switches">
      <legend>{t('maison.groupes_stores.interrupteurs')}</legend>
      <p class="help">{t('maison.groupes_stores.interrupteurs_aide')}</p>
      {#each switches as sw (sw.device)}
        <div class="switch">
          <div class="switch-head">
            <span class="who">
              <b>{device(sw.device) ? nameOf(sw.device) : sw.device}</b>
              {#if !device(sw.device)}<small>{t('commun.volet.injoignable')}</small>{/if}
            </span>
            <button type="button" class="close" onclick={() => removeSwitch(sw.device)} aria-label={t('maison.groupes_stores.retirer_interrupteur')}><Icon name="close" size={18} /></button>
          </div>
          {#each SLOTS as slot (slot)}
            {@const waiting = learning?.device === sw.device && learning.slot === slot}
            <div class="slot">
              <span class="slot-name">{t(`maison.groupes_stores.${SLOT_WORD[slot]}`)}</span>
              <span class="press" class:none={!sw[slot]} class:waiting>{waiting ? t('maison.groupes_stores.appuyez') : pressWord(sw[slot])}</span>
              {#if sw[slot] && !waiting}
                <button type="button" class="close" onclick={() => setPress(sw.device, slot, null)} aria-label={t('maison.groupes_stores.effacer_touche')}><Icon name="close" size={16} /></button>
              {/if}
              <button type="button" class="btn small" class:main={waiting} onclick={() => learn(sw.device, slot)}>{waiting ? t('commun.annuler') : t('maison.groupes_stores.apprendre')}</button>
            </div>
          {/each}
        </div>
      {/each}
      {#if !switchesRead}
        <p class="help">…</p>
      {:else if addable.length}
        <select class="add" value="" onchange={(e) => { addSwitch(e.currentTarget.value); e.currentTarget.value = ''; }}>
          <option value="">{t('maison.groupes_stores.ajouter_interrupteur')}</option>
          {#each addable as r (r.id)}
            <option value={r.id}>{r.name}{r.room ? ` · ${r.room}` : ''}</option>
          {/each}
        </select>
      {:else if !switches.length}
        <p class="help">{t('maison.groupes_stores.aucun_interrupteur')}</p>
      {/if}
    </fieldset>

    <p class="help">{t('maison.groupes_stores.aide')}</p>
    {#if guarded}<p class="help warn">{t('maison.groupes_stores.protege')}</p>{/if}

    <footer>
      {#if id}
        <button type="button" class="btn danger" disabled={busy} onclick={remove}>{t('maison.groupes_stores.supprimer')}</button>
      {/if}
      <span class="push"></span>
      <button type="button" class="btn" onclick={() => dialog.close()}>{t('commun.annuler')}</button>
      <button type="submit" class="btn main" disabled={!ready}>{t(id ? 'maison.groupes_stores.enregistrer' : 'maison.groupes_stores.creer_court')}</button>
    </footer>
  </form>
</dialog>

<style>
  dialog {
    border: 0;
    padding: 22px;
    width: min(480px, 96vw);
    max-height: 94dvh;
    overflow-y: auto;
    border-radius: var(--r-xl);
    background: var(--surface);
    color: var(--ink);
    box-shadow: 0 30px 80px rgb(0 0 0 / 40%);
  }

  dialog::backdrop {
    background: rgb(4 6 12 / 60%);
    backdrop-filter: blur(4px);
  }

  form {
    display: grid;
    gap: 14px;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  h2 {
    margin: 0;
  }

  .close {
    border: 0;
    background: transparent;
    color: var(--ink-2);
    cursor: pointer;
    padding: 4px;
  }

  .name {
    display: grid;
    gap: 6px;
    font-size: 13px;
    font-weight: 650;
    color: var(--ink-2);
  }

  .name input {
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--surface-2);
    color: var(--ink);
    font: inherit;
    font-size: 16px;
    font-weight: 500;
  }

  fieldset {
    margin: 0;
    padding: 0;
    border: 0;
    display: grid;
  }

  legend {
    padding: 0 0 6px;
    font-size: 13px;
    font-weight: 650;
    color: var(--ink-2);
  }

  .member {
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 44px;
    padding: 6px 2px;
    border-top: 1px solid var(--line);
    cursor: pointer;
  }

  .member input {
    flex: none;
    width: 20px;
    height: 20px;
    accent-color: var(--good);
  }

  .who {
    min-width: 0;
    display: grid;
  }

  .who b {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .who small {
    font-size: 12px;
    color: var(--ink-3);
  }

  .switches {
    gap: 8px;
  }

  .switch {
    display: grid;
    gap: 2px;
    padding: 8px 10px;
    border: 1px solid var(--line);
    border-radius: 12px;
    background: var(--surface-2);
  }

  .switch-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .slot {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 40px;
    border-top: 1px solid var(--line);
  }

  .slot-name {
    flex: none;
    width: 70px;
    font-size: 14px;
    font-weight: 600;
  }

  .press {
    flex: 1;
    min-width: 0;
    font-size: 13px;
    color: var(--ink-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .press.none {
    color: var(--ink-3);
  }

  .press.waiting {
    color: var(--warm-ink);
    font-weight: 650;
  }

  .btn.small {
    min-height: 32px;
    padding: 4px 12px;
    font-size: 13px;
    background: var(--surface);
  }

  .btn.small.main {
    background: var(--warm-soft);
  }

  .add {
    min-height: 40px;
    padding: 8px 10px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--surface-2);
    color: var(--ink);
    font: inherit;
    font-size: 14px;
  }

  .help {
    margin: 0;
    font-size: 13px;
    color: var(--ink-3);
  }

  .warn {
    color: var(--warm-ink);
  }

  footer {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .push {
    flex: 1;
  }

  .btn {
    min-height: 40px;
    padding: 8px 16px;
    border: 0;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--ink);
    font: inherit;
    font-size: 14px;
    font-weight: 650;
    cursor: pointer;
  }

  .btn.main {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .btn.danger {
    background: none;
    color: var(--ink-3);
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>

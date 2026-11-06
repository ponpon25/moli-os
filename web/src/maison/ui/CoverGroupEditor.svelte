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

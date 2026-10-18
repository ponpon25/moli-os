<script>
  import { onMount } from 'svelte';
  import qrcode from 'qrcode-generator';
  import { home, note } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** Moli as a HomeKit bridge: which devices the Home app sees, and the
   *  setup code to add Moli there. What leaves the house is a person's
   *  choice (code asked); the setup code is shown to a person only. */
  let state = $state(null);
  let open = $state(false);
  let filter = $state('');
  let busy = $state(false);

  async function load() {
    try {
      const res = await fetch('/api/homekit');
      if (res.ok) state = await res.json();
    } catch {
      /* the card stays as it was */
    }
  }
  onMount(load);

  const status = $derived(state?.status);
  const devices = $derived(state?.devices ?? []);
  const shown = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return devices;
    return devices.filter((d) => `${d.name} ${d.room ?? ''} ${d.id}`.toLowerCase().includes(q));
  });
  const qr = $derived.by(() => {
    if (!status?.setup_uri) return '';
    const code = qrcode(0, 'M');
    code.addData(status.setup_uri);
    code.make();
    return code.createSvgTag({ cellSize: 4, margin: 2, scalable: true });
  });

  /** A person's gesture: the code first, then the same request again. */
  async function write(method, path, body, label, again) {
    busy = true;
    try {
      const res = await fetch(path, {
        method,
        headers: { 'content-type': 'application/json', 'x-moli-origin': 'ui' },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      if (res.status === 403) {
        home.held = { label, reason: t('systeme.homekit.code_demande'), custom: again };
        return false;
      }
      const reply = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(reply.error ?? res.statusText);
      await load();
      // The bridge rebuilds its database a moment after a change.
      setTimeout(load, 900);
      return true;
    } catch (e) {
      note(t('systeme.homekit.erreur', { message: e.message }), 'error');
      return false;
    } finally {
      busy = false;
    }
  }

  const setPolicy = (policy) =>
    write('PUT', '/api/homekit/policy', { policy }, t('systeme.homekit.titre'), () => setPolicy(policy));

  const setDevice = (d, exposed) =>
    write(
      'PUT',
      `/api/homekit/devices/${encodeURIComponent(d.id)}`,
      { exposed },
      d.name,
      () => setDevice(d, exposed),
    );

  async function unpair() {
    if (!confirm(t('systeme.homekit.reinitialiser_confirme'))) return;
    const done = await write('POST', '/api/homekit/unpair', undefined, t('systeme.homekit.titre'), unpair);
    if (done) note(t('systeme.homekit.reinitialise'), 'info');
  }

  /** Asks for the code, then reads the page again (the setup code comes
   *  with a person's session). */
  function reveal() {
    home.held = { label: t('systeme.homekit.titre'), reason: t('systeme.homekit.code_demande'), custom: load };
  }
</script>

{#if state?.enabled && status}
  <section class="keys">
    <div class="row">
      <span class="ico"><Icon name={status.controllers ? 'ok' : 'lock'} size={18} /></span>
      <div class="text">
        <b>{t('systeme.homekit.titre')}</b>
        <span class="muted">
          {t('systeme.homekit.resume', { devices: status.exposed, count: status.controllers })}
        </span>
      </div>
      <button class="btn" onclick={() => (open = !open)}>
        {open ? t('systeme.homekit.fermer') : t('systeme.homekit.regler')}
      </button>
    </div>

    {#if open}
      <div class="pair">
        {#if status.setup_code}
          <div class="qr">{@html qr}</div>
          <div class="code">
            <span class="muted">{t('systeme.homekit.code_titre')}</span>
            <code>{status.setup_code}</code>
            <small class="muted">{t('systeme.homekit.code_aide', { name: status.name })}</small>
          </div>
        {:else}
          <div class="code">
            <small class="muted">{t('systeme.homekit.code_cache')}</small>
            <button class="btn" onclick={reveal}>{t('systeme.homekit.afficher_code')}</button>
          </div>
        {/if}
        {#if status.controllers}
          <button class="btn danger" disabled={busy} onclick={unpair}>{t('systeme.homekit.reinitialiser')}</button>
        {/if}
      </div>

      <div class="policy" role="radiogroup" aria-label={t('systeme.homekit.par_defaut')}>
        <span class="muted">{t('systeme.homekit.par_defaut')}</span>
        <label class:on={status.policy === 'all'}>
          <input type="radio" name="hk-policy" checked={status.policy === 'all'} disabled={busy} onchange={() => setPolicy('all')} />
          {t('systeme.homekit.tout')}
        </label>
        <label class:on={status.policy === 'none'}>
          <input type="radio" name="hk-policy" checked={status.policy === 'none'} disabled={busy} onchange={() => setPolicy('none')} />
          {t('systeme.homekit.rien')}
        </label>
      </div>

      {#if status.truncated}
        <p class="warn">{t('systeme.homekit.trop')}</p>
      {/if}

      <input
        class="filter"
        type="search"
        placeholder={t('systeme.homekit.filtrer')}
        bind:value={filter}
        spellcheck="false"
        autocomplete="off"
      />

      <ul class="devices">
        {#each shown as d (d.id)}
          <li class:off={!d.exposed}>
            <label>
              <input type="checkbox" checked={d.exposed} disabled={busy} onchange={(e) => setDevice(d, e.currentTarget.checked)} />
              <span class="name">{d.name}</span>
            </label>
            {#if d.room}<span class="muted room">{d.room}</span>{/if}
            <span class="kinds">
              {#each d.kinds as k (k)}<span class="chip">{t('systeme.homekit.type.' + k)}</span>{/each}
              {#if d.absent}<span class="chip warm">{t('systeme.homekit.absent')}</span>{/if}
            </span>
            {#if d.exception}
              <button class="link" disabled={busy} title={t('systeme.homekit.suivre_defaut')} onclick={() => setDevice(d, null)}>↺</button>
            {/if}
          </li>
        {:else}
          <li class="muted">{t('systeme.homekit.aucun')}</li>
        {/each}
      </ul>
      <small class="muted">{t('systeme.homekit.garde')}</small>
    {/if}
  </section>
{/if}

<style>
  .keys {
    display: grid;
    gap: 14px;
    padding: 16px 18px;
    border-radius: var(--r-md);
    background: var(--surface-2);
  }

  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }

  .ico {
    width: 36px;
    height: 36px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--surface);
    color: var(--warm-ink);
  }

  .text {
    display: grid;
    gap: 2px;
    flex: 1;
    min-width: 220px;
  }

  .text .muted {
    font-size: 13px;
  }

  .btn {
    border: 0;
    border-radius: 999px;
    padding: 9px 16px;
    font: inherit;
    font-weight: 650;
    background: var(--surface);
    color: var(--ink);
    cursor: pointer;
  }

  .btn.danger {
    color: var(--bad, #c0392b);
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .pair {
    display: flex;
    gap: 18px;
    align-items: center;
    flex-wrap: wrap;
  }

  .qr {
    width: 150px;
    padding: 6px;
    border-radius: var(--r-sm);
    /* a QR code is scanned dark on light, whatever the theme */
    background: #fff;
  }

  .code {
    display: grid;
    gap: 6px;
    flex: 1;
    min-width: 200px;
  }

  .code code {
    font-size: 26px;
    font-weight: 700;
    letter-spacing: 0.08em;
  }

  .policy {
    display: flex;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
    font-size: 14px;
  }

  .policy label {
    display: flex;
    gap: 6px;
    align-items: center;
    padding: 6px 12px;
    border-radius: 999px;
    background: var(--surface);
    cursor: pointer;
  }

  .policy label.on {
    background: var(--warm);
    color: #fff;
  }

  .policy input {
    accent-color: currentColor;
  }

  .warn {
    margin: 0;
    padding: 8px 12px;
    border-radius: var(--r-sm);
    background: var(--warm-soft);
    font-size: 13px;
  }

  .filter {
    font: inherit;
    font-size: 14px;
    padding: 9px 12px;
    border-radius: var(--r-sm);
    border: 1px solid var(--line);
    background: var(--surface);
    color: var(--ink);
  }

  .devices {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 420px;
    overflow: auto;
    display: grid;
    gap: 2px;
  }

  .devices li {
    display: flex;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
    padding: 6px 8px;
    border-radius: var(--r-sm);
  }

  .devices li:hover {
    background: var(--surface);
  }

  .devices li.off .name {
    color: var(--ink-2);
    text-decoration: line-through;
  }

  .devices label {
    display: flex;
    gap: 8px;
    align-items: center;
    flex: 1;
    min-width: 180px;
    cursor: pointer;
  }

  .room {
    font-size: 12px;
  }

  .kinds {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }

  .chip {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--surface);
    color: var(--ink-2);
  }

  .chip.warm {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .link {
    border: 0;
    background: none;
    color: var(--ink-2);
    cursor: pointer;
    font-size: 16px;
  }

  small {
    font-size: 12px;
  }
</style>

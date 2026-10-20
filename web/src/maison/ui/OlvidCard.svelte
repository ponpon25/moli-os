<script>
  import qrcode from 'qrcode-generator';
  import { hub, value, act, note } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** Olvid, the house's private messenger: who it writes to, and the dance
   *  that adds someone (open the link in Olvid, compare two short codes).
   *  Opening the pairing, and every step after, asks for the household code. */
  const bot = $derived(Object.values(hub.devices).find((d) => d.manufacturer === 'Olvid'));
  const driver = $derived(hub.drivers.find((d) => d.kind === 'olvid' || d.instance.startsWith('olvid')));
  const id = $derived(bot?.id);

  const status = $derived(value(id, 'status'));
  const pairing = $derived(value(id, 'pairing') === true);
  const link = $derived(value(id, 'link') || '');
  const invitation = $derived(value(id, 'invitation') || '');
  const code = $derived(value(id, 'code') || '');
  const people = $derived(
    String(value(id, 'contacts') || '')
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean),
  );

  const qr = $derived.by(() => {
    if (!link) return '';
    const q = qrcode(0, 'M');
    q.addData(link);
    q.make();
    return q.createSvgTag({ cellSize: 4, margin: 2, scalable: true });
  });

  let typed = $state('');
  let busy = $state(false);

  async function run(point, val, label) {
    busy = true;
    try {
      return await act(`${id}/${point}`, val, label);
    } finally {
      busy = false;
    }
  }

  async function validate(e) {
    e.preventDefault();
    if ((await run('sas', typed, t('systeme.olvid.valider'))) === 'ok') typed = '';
  }

  async function copy() {
    try {
      await navigator.clipboard.writeText(link);
      note(t('systeme.olvid.lien_copie'));
    } catch {
      /* the link stays on screen to be copied by hand */
    }
  }
</script>

{#if bot}
  <section class="card olvid">
    <div class="head">
      <span class="ico"><Icon name="lock" size={20} /></span>
      <div class="text">
        <b>Olvid</b>
        <span class="muted">{status}</span>
      </div>
      {#if !pairing}
        <button class="btn" disabled={busy} onclick={() => run('pairing', true, t('systeme.olvid.appairer'))}>
          <Icon name="plus" size={16} />{t('systeme.olvid.appairer')}
        </button>
      {:else}
        <button class="btn" disabled={busy} onclick={() => run('pairing', false, t('systeme.olvid.annuler'))}>{t('systeme.olvid.annuler')}</button>
      {/if}
    </div>

    {#if people.length}
      <ul class="people">
        {#each people as name (name)}
          <li>
            {name}
            <button aria-label={t('systeme.olvid.oublier', { name })} title={t('systeme.olvid.oublier', { name })} disabled={busy} onclick={() => run('forget', name, t('systeme.olvid.oublier', { name }))}>
              <Icon name="close" size={14} />
            </button>
          </li>
        {/each}
      </ul>
      <div class="actions">
        <button class="btn" disabled={busy} onclick={() => run('notify', t('systeme.olvid.test_texte'), t('systeme.olvid.test'))}>{t('systeme.olvid.test')}</button>
      </div>
    {/if}

    {#if pairing}
      <ol class="steps">
        <li>
          <b>{t('systeme.olvid.etape_lien')}</b>
          {#if link}
            <div class="pair">
              <div class="qr">{@html qr}</div>
              <div class="link">
                <code>{link}</code>
                <button class="btn" onclick={copy}>{t('systeme.olvid.copier')}</button>
              </div>
            </div>
          {/if}
        </li>
        <li class:off={!invitation}>
          <b>{t('systeme.olvid.etape_code_bot')}</b>
          {#if invitation && code}
            <span class="big">{code}</span>
            <small class="muted">{t('systeme.olvid.pour', { name: invitation })}</small>
          {:else}
            <small class="muted">{t('systeme.olvid.attente')}</small>
          {/if}
        </li>
        <li class:off={!invitation}>
          <b>{t('systeme.olvid.etape_code_olvid')}</b>
          <form onsubmit={validate}>
            <input bind:value={typed} inputmode="numeric" autocomplete="off" maxlength="12" placeholder="0000" aria-label={t('systeme.olvid.etape_code_olvid')} disabled={!invitation} />
            <button class="btn primary" type="submit" disabled={busy || !typed.trim() || !invitation}>{t('systeme.olvid.valider')}</button>
          </form>
        </li>
      </ol>
    {/if}
  </section>
{:else if driver}
  <section class="card olvid">
    <div class="head">
      <span class="ico"><Icon name="lock" size={20} /></span>
      <div class="text">
        <b>Olvid</b>
        <span class="muted">{driver.status?.reason || driver.status?.error || t('systeme.olvid.demarrage')}</span>
      </div>
    </div>
  </section>
{/if}

<style>
  .olvid {
    display: grid;
    gap: 16px;
  }

  .head {
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
    background: var(--surface-2);
    color: var(--warm-ink);
  }

  .text {
    display: grid;
    gap: 2px;
    flex: 1;
    min-width: 220px;
  }

  .muted {
    font-size: 13px;
    color: var(--ink-3);
  }

  .btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 0;
    border-radius: 999px;
    padding: 9px 16px;
    font: inherit;
    font-weight: 650;
    background: var(--surface-2);
    color: var(--ink);
    cursor: pointer;
  }

  .btn.primary {
    background: var(--warm-ink);
    color: #fff;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .people {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .people li {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 5px 6px 5px 14px;
    border-radius: 999px;
    font-weight: 650;
    background: var(--surface-2);
  }

  .people button {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--ink-3);
    cursor: pointer;
  }

  .steps {
    display: grid;
    gap: 16px;
    margin: 0;
    padding: 16px 0 0 20px;
    border-top: 1px solid var(--line);
  }

  .steps li {
    display: grid;
    gap: 8px;
  }

  .steps li.off {
    opacity: 0.5;
  }

  .pair {
    display: flex;
    flex-wrap: wrap;
    gap: 16px;
    align-items: center;
  }

  .qr {
    width: 150px;
    height: 150px;
    padding: 6px;
    border-radius: 12px;
    background: #fff;
  }

  .qr :global(svg) {
    width: 100%;
    height: 100%;
  }

  .link {
    display: grid;
    gap: 8px;
    justify-items: start;
    min-width: 0;
    flex: 1;
  }

  .link code {
    max-width: 100%;
    overflow-wrap: anywhere;
    font-size: 12px;
    color: var(--ink-2);
  }

  .big {
    font-size: 34px;
    font-weight: 750;
    letter-spacing: 0.2em;
    font-variant-numeric: tabular-nums;
  }

  form {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }

  input {
    width: 130px;
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 9px 16px;
    font: inherit;
    font-size: 18px;
    letter-spacing: 0.15em;
    background: var(--surface);
    color: var(--ink);
  }
</style>

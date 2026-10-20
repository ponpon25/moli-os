<script>
  import { onMount } from 'svelte';
  import { hub, decide, login, logout } from '../../lib/hub.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** The guard, in one place: who is recognised as a human, the orders it
   *  held for a human to release, the protected rooms and the quiet hours. */

  const WHO = ['household', 'mcp', 'api', 'cli', 'assistant'];
  const who = (origin) => (WHO.includes(origin) ? t(`systeme.approbations.origine.${origin}`) : origin);

  let now = $state(Date.now());
  onMount(() => {
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });

  // Name and rooms are frozen in the request: renaming the device afterwards
  // cannot disguise what is approved.
  function describe(request) {
    const i = request.point.indexOf('/');
    const device = hub.devices[request.point.slice(0, i)];
    const key = request.point.slice(i + 1);
    const point = device?.points.find((p) => p.key === key);
    let action = `${point?.label ?? key} → ${JSON.stringify(request.value)}`;
    if (point?.kind.type === 'binary') action = request.value ? t('systeme.approbations.allumer') : t('systeme.approbations.eteindre');
    const target =
      request.native_name && request.native_name !== request.device_name ? `${request.device_name} (${request.native_name})` : request.device_name;
    return { target, action, room: request.rooms.join(' / ') };
  }

  function remaining(request) {
    const s = Math.max(0, Math.round((request.expires - now) / 1000));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  }

  let pin = $state('');
  let error = $state('');
  let busy = $state(false);
  async function submit(e) {
    e.preventDefault();
    busy = true;
    error = (await login(pin)) ?? '';
    busy = false;
    pin = '';
  }

  const waiting = $derived(hub.drivers.filter((d) => d.status.state === 'waiting'));
</script>

<div class="desk">
  {#each hub.guard?.unmatched_protected_rooms ?? [] as room (room)}
    <p class="warn" role="alert"><Icon name="alert" size={18} /><span><b>{t('commun.atelier.piece_introuvable', { room })}</b> {t('commun.atelier.piece_introuvable_detail')}</span></p>
  {/each}
  {#each waiting as d (d.instance)}
    <p class="warn" role="alert"><Icon name="alert" size={18} /><span><b>{d.instance}</b> {d.status.reason}</span></p>
  {/each}

  <section class="card">
    <h2>{t('systeme.securite.presence')}</h2>
    {#if hub.session.human}
      <p class="line ok"><Icon name="check" size={18} />{t('systeme.securite.reconnu')}</p>
      <p class="muted">{t('systeme.session.ouverte')}</p>
      <button class="soft" onclick={logout}>{t('systeme.securite.quitter')}</button>
    {:else if hub.session.pin_configured}
      <p class="muted">{t('systeme.session.aide')}</p>
      <form onsubmit={submit}>
        <input type="password" inputmode="numeric" autocomplete="off" bind:value={pin} placeholder={t('systeme.session.code')} aria-label={t('systeme.session.code_tableau')} />
        <button type="submit" class="primary" disabled={busy || !pin}>{t('systeme.session.je_suis_la')}</button>
      </form>
      {#if error}<p class="error" role="alert">{error}</p>{/if}
    {:else}
      <p class="muted">{t('systeme.securite.pas_de_code')}</p>
    {/if}
  </section>

  <section class="card">
    <h2>{t('systeme.securite.demandes')} {#if hub.approvals.length}<span class="count">{hub.approvals.length}</span>{/if}</h2>
    {#each hub.approvals as request (request.id)}
      {@const d = describe(request)}
      <article>
        <p>
          <b>{who(request.origin)}</b>{#if request.actor} <span class="muted">{t('systeme.approbations.declare', { actor: request.actor })}</span>{/if}
          {t('systeme.approbations.veut')} <b>{d.action}</b> · {d.target}{#if d.room} · {d.room}{/if}
        </p>
        <p class="muted small">{t('systeme.approbations.retenu', { reason: request.reason })} <span class="num">{remaining(request)}</span></p>
        <div class="buttons">
          {#if hub.session.human}
            <button class="primary" onclick={() => decide(request, true)}>{t('systeme.approbations.autoriser')}</button>
            <button class="soft" onclick={() => decide(request, false)}>{t('systeme.approbations.refuser')}</button>
          {:else}
            <span class="muted small">{t('systeme.approbations.indice')}</span>
          {/if}
        </div>
      </article>
    {:else}
      <p class="muted">{t('systeme.securite.aucune_demande')}</p>
    {/each}
  </section>

  <section class="card">
    <h2>{t('systeme.securite.pieces')}</h2>
    <p class="muted">{t('systeme.securite.pieces_aide')}</p>
    {#if hub.guard?.protected_rooms?.length}
      <ul class="chips">
        {#each hub.guard.protected_rooms as room (room)}<li><Icon name="lock" size={14} />{room}</li>{/each}
      </ul>
    {:else}
      <p class="muted">{t('systeme.securite.aucune_piece')}</p>
    {/if}
    {#if hub.guard?.quiet_hours?.active}
      <p class="line"><Icon name="moon" size={18} />{t('systeme.entete.heures_calmes', { to: hub.guard.quiet_hours.to })}</p>
    {/if}
  </section>
</div>

<style>
  .desk {
    display: grid;
    gap: 16px;
  }

  h2 {
    margin: 0 0 10px;
    font-size: 16px;
  }

  .count {
    margin-left: 6px;
    padding: 1px 9px;
    border-radius: 999px;
    font-size: 13px;
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .warn {
    display: flex;
    gap: 10px;
    margin: 0;
    padding: 12px 16px;
    border-radius: 16px;
    font-size: 14px;
    background: color-mix(in srgb, var(--alert) 14%, var(--surface));
    color: var(--ink);
  }

  .line {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 10px 0 0;
    font-weight: 650;
  }

  .line.ok {
    color: #2f9e55;
    margin: 0 0 6px;
  }

  p {
    margin: 0 0 8px;
  }

  .small {
    font-size: 13px;
  }

  .error {
    color: var(--alert);
    font-size: 13px;
  }

  form {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }

  input {
    flex: 1;
    min-width: 140px;
    max-width: 240px;
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 9px 16px;
    font: inherit;
    background: var(--surface);
    color: var(--ink);
  }

  .primary,
  .soft {
    border: 0;
    border-radius: 999px;
    padding: 9px 18px;
    font: inherit;
    font-size: 14px;
    font-weight: 650;
    cursor: pointer;
  }

  .primary {
    background: var(--warm-ink);
    color: #fff;
  }

  .soft {
    background: var(--surface-2);
    color: var(--ink-2);
  }

  .primary:disabled {
    opacity: 0.5;
    cursor: default;
  }

  article {
    padding: 12px 0;
    border-top: 1px solid var(--line);
  }

  article:first-of-type {
    border-top: 0;
  }

  .buttons {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin: 4px 0 0;
    padding: 0;
    list-style: none;
  }

  .chips li {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    border-radius: 999px;
    font-size: 13.5px;
    font-weight: 650;
    background: var(--surface-2);
    color: var(--ink-2);
  }
</style>

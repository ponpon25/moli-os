<script>
  import { onMount } from 'svelte';
  import { device, value, act, pending, home, relative, num } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** The 433 MHz transceiver's own page: what it is, what it listens to,
   *  and « listen to everything » for five minutes, with who spoke. To find
   *  out whether a remote (a garage door, a doorbell) is heard at all. */
  let { instance, onclose } = $props();

  // A second hand for the countdown (home.now ticks every 15 s).
  let clock = $state(Date.now());

  const id = $derived(`${instance}:transceiver`);
  const d = $derived(device(id));
  const listening = $derived(value(id, 'listen') === true);
  const until = $derived(value(id, 'listen_until'));
  const left = $derived(listening && typeof until === 'number' ? Math.max(0, until - clock) : 0);
  const protocols = $derived(String(value(id, 'protocols') ?? '').split(', ').filter(Boolean));
  const heard = $derived.by(() => {
    try {
      const list = JSON.parse(value(id, 'heard') ?? '[]');
      return Array.isArray(list) ? list : [];
    } catch {
      return [];
    }
  });

  let dialog;
  onMount(() => {
    dialog.showModal();
    const timer = setInterval(() => (clock = Date.now()), 1000);
    return () => clearInterval(timer);
  });

  const mmss = (ms) => {
    const s = Math.ceil(ms / 1000);
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  };
  const ICON = {
    sensor: 'thermometer',
    lighting: 'light',
    lighting4: 'remote',
    remote_control: 'remote',
    chime: 'bell',
    security: 'shield',
    blinds: 'shutter',
    undecoded: 'flask',
    other: 'info',
  };
  const bars = (rssi) => Math.max(1, Math.min(4, Math.round((rssi / 15) * 4)));
</script>

<dialog bind:this={dialog} {onclose} onclick={(e) => e.target === dialog && dialog.close()} aria-label={t('maison.rfx.titre')}>
  <header>
    <div>
      <p class="eyebrow">{t('maison.rfx.surtitre')}</p>
      <h2>{t('maison.rfx.titre')}</h2>
      {#if d}<p class="muted fw">{value(id, 'firmware')}</p>{/if}
    </div>
    <button class="close" onclick={() => dialog.close()} aria-label={t('maison.rfx.fermer')}><Icon name="close" size={22} /></button>
  </header>

  {#if !d}
    <p class="muted">{t('maison.rfx.absente')}</p>
  {:else}
    <section class="listen" class:on={listening}>
      <div class="radar" aria-hidden="true">
        <i></i><i></i><i></i>
        <span><Icon name="remote" size={26} /></span>
      </div>
      <div class="text">
        {#if listening}
          <h3>{t('maison.rfx.ecoute_en_cours')} <span class="num">{mmss(left)}</span></h3>
          <p>{t('maison.rfx.ecoute_aide')}</p>
        {:else}
          <h3>{t('maison.rfx.ecouter_titre')}</h3>
          <p>{t('maison.rfx.ecouter_aide')}</p>
        {/if}
      </div>
      <button class="go" disabled={pending(id, 'listen')} onclick={() => act(`${id}/listen`, !listening, t('maison.rfx.ecoute'))}>
        {listening ? t('maison.rfx.arreter') : t('maison.rfx.ecouter')}
      </button>
    </section>

    {#if heard.length || listening}
      <h3 class="sub">{t('maison.rfx.entendu', { n: heard.length })}</h3>
      {#if !heard.length}
        <p class="muted wait">{t('maison.rfx.rien_encore')}</p>
      {:else}
        <ul class="heard">
          {#each heard as h (h.packet + h.code)}
            <li class={h.standing}>
              <span class="glyph"><Icon name={ICON[h.kind] ?? 'info'} size={20} /></span>
              <span class="who">
                <b>{h.model ?? t(`maison.rfx.type.${h.kind}`)}</b>
                <small><code>{h.code}</code>{#if h.reading} · {h.reading}{/if}</small>
              </span>
              <span class="meta">
                <span class="chip {h.standing}">{t(`maison.rfx.etat.${h.standing}`)}</span>
                <small class="num">
                  {t('maison.rfx.fois', { n: num(h.count, 0) })} · {relative(h.last, Math.max(home.now, clock))}
                  <span class="bars" aria-label={t('maison.rfx.signal', { n: bars(h.rssi) })}>
                    {#each [1, 2, 3, 4] as b (b)}<i class:lit={b <= bars(h.rssi)} style="height: {b * 25}%"></i>{/each}
                  </span>
                </small>
              </span>
            </li>
          {/each}
        </ul>
        <p class="muted note">{t('maison.rfx.note')}</p>
      {/if}
    {/if}

    <h3 class="sub">{t('maison.rfx.protocoles')}</h3>
    <div class="protocols">
      {#each protocols as p (p)}<span>{p}</span>{/each}
    </div>
    <p class="muted note">{t('maison.rfx.protocoles_aide')}</p>
  {/if}
</dialog>

<style>
  dialog {
    border: 0;
    padding: 22px;
    width: min(720px, 96vw);
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

  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 16px;
  }

  .eyebrow {
    margin: 0;
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--ink-3);
  }

  h2 {
    margin: 2px 0 0;
    font-size: 24px;
    font-weight: 700;
    letter-spacing: -0.02em;
  }

  .fw {
    margin: 4px 0 0;
    font-size: 13px;
  }

  .close {
    border: 0;
    background: var(--surface-2);
    color: var(--ink-2);
    border-radius: 999px;
    width: 40px;
    height: 40px;
    display: grid;
    place-items: center;
    cursor: pointer;
    flex: none;
  }

  .listen {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 16px;
    padding: 16px;
    border-radius: var(--r-lg);
    background: var(--surface-2);
    transition: background 0.4s var(--ease);
  }

  .listen.on {
    background: var(--warm-soft);
  }

  .radar {
    position: relative;
    width: 64px;
    height: 64px;
    display: grid;
    place-items: center;
  }

  .radar span {
    position: relative;
    width: 44px;
    height: 44px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--surface);
    color: var(--ink-3);
    box-shadow: var(--shadow);
  }

  .on .radar span {
    color: var(--warm-ink);
  }

  .radar i {
    position: absolute;
    inset: 10px;
    border-radius: 50%;
    border: 2px solid var(--warm);
    opacity: 0;
  }

  .on .radar i {
    animation: wave 2.4s var(--ease) infinite;
  }

  .on .radar i:nth-child(2) {
    animation-delay: 0.8s;
  }

  .on .radar i:nth-child(3) {
    animation-delay: 1.6s;
  }

  h3 {
    margin: 0;
    font-size: 16px;
    font-weight: 700;
  }

  .text p {
    margin: 4px 0 0;
    font-size: 13px;
    color: var(--ink-2);
  }

  .go {
    border: 0;
    border-radius: 999px;
    padding: 11px 18px;
    font: inherit;
    font-weight: 700;
    font-size: 14px;
    background: var(--ink);
    color: var(--surface);
    cursor: pointer;
    white-space: nowrap;
  }

  .on .go {
    background: var(--surface);
    color: var(--ink);
  }

  .sub {
    margin: 22px 0 10px;
    font-size: 13px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ink-3);
  }

  .wait {
    margin: 0;
  }

  .heard {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 8px;
  }

  .heard li {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border-radius: var(--r-md);
    background: var(--surface-2);
    animation: arrive 0.4s var(--ease);
  }

  .glyph {
    width: 38px;
    height: 38px;
    border-radius: 12px;
    display: grid;
    place-items: center;
    background: var(--surface);
    color: var(--ink-2);
  }

  .unread .glyph {
    color: var(--warm-ink);
  }

  .who {
    display: grid;
    min-width: 0;
  }

  .who b {
    font-size: 14px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .who small {
    font-size: 12px;
    color: var(--ink-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  code {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11.5px;
  }

  .meta {
    display: grid;
    justify-items: end;
    gap: 4px;
  }

  .meta small {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
    color: var(--ink-3);
    white-space: nowrap;
  }

  .chip {
    font-size: 11.5px;
    font-weight: 700;
    padding: 3px 9px;
    border-radius: 999px;
    background: var(--surface);
    color: var(--ink-3);
  }

  .chip.known {
    background: var(--good-soft);
    color: var(--good);
  }

  .chip.new {
    background: var(--cool-soft);
    color: var(--cool);
  }

  .chip.unread {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .bars {
    display: inline-flex;
    align-items: flex-end;
    gap: 2px;
    height: 10px;
  }

  .bars i {
    width: 3px;
    border-radius: 2px;
    background: color-mix(in oklch, var(--ink-3) 30%, transparent);
  }

  .bars i.lit {
    background: var(--ink-3);
  }

  .note {
    margin: 10px 2px 0;
    font-size: 12.5px;
    line-height: 1.45;
  }

  .protocols {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .protocols span {
    font-size: 12.5px;
    font-weight: 650;
    padding: 5px 11px;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--ink-2);
  }

  @keyframes wave {
    0% {
      transform: scale(0.8);
      opacity: 0.9;
    }
    100% {
      transform: scale(1.9);
      opacity: 0;
    }
  }

  @keyframes arrive {
    from {
      transform: translateY(-6px);
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .on .radar i,
    .heard li {
      animation: none;
    }
  }

  @media (max-width: 560px) {
    .listen {
      grid-template-columns: auto 1fr;
    }

    .go {
      grid-column: 1 / -1;
    }

    .heard li {
      grid-template-columns: auto 1fr;
    }

    .meta {
      grid-column: 2;
      justify-items: start;
    }
  }
</style>

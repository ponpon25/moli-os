<script>
  /** The way between pages, the same on every one: a strip of glass at the foot
   *  of the window, and Moli one tap away (the home page has its own bar). */
  import { NAV } from '../lib/nav.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';
  import Orb from '../moli/Orb.svelte';

  let { page, moli = false, busy = false, onask } = $props();

  const known = $derived(NAV.some((n) => n.path === page) || ['reglages', 'systeme'].includes(page));
  const links = [...NAV.map((n) => ({ ...n, match: [n.path] })), { path: 'reglages', label: 'commun.nav.systeme', icon: 'tune', match: ['reglages', 'systeme'] }];
  const isActive = (l) => l.match.includes(page) || (!known && l.path === '' && !page);

  // On a phone the strip scrolls: the page we are on stays in view.
  let strip = $state();
  $effect(() => {
    void page;
    strip?.querySelector('.active')?.scrollIntoView({ inline: 'center', block: 'nearest' });
  });
</script>

<nav class="dock" aria-label={t('commun.nav.aria')}>
  {#if moli}
    <button class="ask" onclick={onask} aria-label={t('commun.demander_moli')} title={t('commun.demander_moli')}><Orb size={30} state={busy ? 'thinking' : 'idle'} /></button>
  {/if}
  <ul bind:this={strip}>
    {#each links as l (l.path)}
      <li>
        <a href="#/{l.path}" class:active={isActive(l)} aria-current={isActive(l) ? 'page' : undefined} title={t(l.label)}>
          <Icon name={l.icon} size={18} />
          <span>{t(l.label)}</span>
        </a>
      </li>
    {/each}
  </ul>
</nav>

<style>
  .dock {
    position: fixed;
    left: 50%;
    bottom: 16px;
    transform: translateX(-50%);
    z-index: 25;
    display: flex;
    align-items: center;
    gap: 4px;
    max-width: calc(100vw - 24px);
    padding: 5px;
    border-radius: 999px;
    background: rgb(16 22 38 / 62%);
    backdrop-filter: blur(22px);
    box-shadow: inset 0 0 0 1px rgb(255 255 255 / 12%), 0 16px 40px -14px rgb(0 0 0 / 60%);
  }

  ul {
    list-style: none;
    display: flex;
    gap: 1px;
    margin: 0;
    padding: 0;
    min-width: 0;
    overflow-x: auto;
    scrollbar-width: none;
  }

  a {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 11px;
    border-radius: 999px;
    color: rgb(255 255 255 / 78%);
    font-size: 13.5px;
    font-weight: 560;
    text-decoration: none;
    white-space: nowrap;
    transition: color 0.2s var(--ease), background 0.2s var(--ease);
  }

  a:hover {
    color: #fff;
    background: rgb(255 255 255 / 10%);
  }

  a.active {
    color: #fff;
    background: rgb(255 255 255 / 22%);
  }

  .ask {
    flex: none;
    display: grid;
    place-items: center;
    padding: 2px;
    border: 0;
    border-radius: 50%;
    background: none;
  }

  /* Narrower windows: the icons, the name in the tooltip. */
  @media (max-width: 1240px) {
    span {
      display: none;
    }

    a {
      padding: 10px;
    }
  }

  /* A phone: the whole width, each page its icon and name. */
  @media (max-width: 760px) {
    .dock {
      left: 0;
      right: 0;
      bottom: 0;
      transform: none;
      max-width: none;
      padding: 6px 8px calc(6px + env(safe-area-inset-bottom));
      border-radius: 20px 20px 0 0;
      background: rgb(13 17 28 / 92%);
    }

    a {
      flex-direction: column;
      gap: 3px;
      width: 62px;
      padding: 6px 2px;
      border-radius: 14px;
    }

    span {
      display: block;
      max-width: 60px;
      overflow: hidden;
      font-size: 10.5px;
      text-overflow: ellipsis;
    }
  }
</style>

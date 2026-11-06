<script>
  import '@fontsource-variable/figtree';
  import '@fontsource-variable/fraunces/wght.css';
  import '@fontsource-variable/fraunces/wght-italic.css';
    import { onMount } from 'svelte';
  import { home, hub, start, value, savePref } from '../../lib/home.svelte.js';
  import { t } from '../../../lib/i18n.svelte.js';
  import Icon from '../../ui/Icon.svelte';
  import SkyBackdrop from '../../ui/SkyBackdrop.svelte';
  import PinSheet from '../../ui/PinSheet.svelte';
  import AlertWizard from '../../auto/AlertWizard.svelte';
  import { alerts } from '../../lib/auto.svelte.js';
  import Setup from '../../ui/Setup.svelte';
  import Login from '../../ui/Login.svelte';
  import Accueil from '../../pages/Accueil.svelte';
  import Salon from '../../pages/Salon.svelte';
  import Pieces from '../../pages/Pieces.svelte';
  import Equipements from '../../pages/Equipements.svelte';
  import Enceintes from '../../pages/Enceintes.svelte';
  import Historique from '../../pages/Historique.svelte';
  import Plan from '../../pages/Plan.svelte';
  import Cameras from '../../pages/Cameras.svelte';
  import Stores from '../../pages/Stores.svelte';
  import Energie from '../../pages/Energie.svelte';
  import Impression from '../../pages/Impression.svelte';
  import Dehors from '../../pages/Dehors.svelte';
  import Moli from '../../pages/Moli.svelte';
  import MoliSheet from '../../moli/MoliSheet.svelte';
  import Automatismes from '../../pages/Automatismes.svelte';
  import Systeme from '../../pages/Systeme.svelte';
  import Parler from '../../pages/Parler.svelte';
  import Dock from '../../ui/Dock.svelte';
  import SkyHero from '../../ui/SkyHero.svelte';
  import { moli } from '../../lib/moli.svelte.js';

  let { route } = $props();

  // Shown only if the link stays down a few seconds (a blink is not news).
  let offline = $state(false);
  $effect(() => {
    if (hub.connected) {
      offline = false;
      return;
    }
    const timer = setTimeout(() => (offline = true), 3000);
    return () => clearTimeout(timer);
  });

  let moliOpen = $state(false);
  onMount(() => start());

  // One look, whatever the hour: the sky says the time of day.
  const theme = 'night';

  $effect(() => {
    const root = document.documentElement.style;
    root.background = '#0d1016';
    document.body.style.background = 'transparent';
    return () => {
      root.background = '';
      document.body.style.background = '';
    };
  });

  const page = $derived(route.split('/')[0] ?? '');
  // Every page but these falls back to the home page, which has the whole
  // window (the sky) and its own way to the others, at its foot.
  const PAGES = new Set(['salon', 'pieces', 'historique', 'equipements', 'enceintes', 'plan', 'carte', 'cameras', 'stores', 'parler', 'atelier', 'energie', 'impression', 'dehors', 'moli', 'automatismes', 'systeme', 'reglages', 'infra']);
  const isHome = $derived(!PAGES.has(page));
</script>

<div class="maison" data-look="ciel" class:home={isHome} data-theme={theme}>
  <SkyBackdrop calm={!isHome} />
  <main class="content">
    <SkyHero>
    <div class="page" class:bare={isHome}>
    {#if page === 'salon'}
      <Salon />
    {:else if page === 'pieces'}
      <Pieces {route} />
    {:else if page === 'historique'}
      <Historique />
    {:else if page === 'equipements'}
      <Equipements />
    {:else if page === 'enceintes'}
      <Enceintes {route} />
    {:else if page === 'plan'}
      <Plan />
    {:else if page === 'carte'}
      <!-- The map (Leaflet): loaded only when visited. -->
      {#await import('../../pages/Carte.svelte') then { default: Carte }}<Carte />{/await}
    {:else if page === 'cameras'}
      <Cameras />
    {:else if page === 'stores'}
      <Stores />
    {:else if page === 'parler'}
      <Parler />
    {:else if page === 'atelier'}
      <Systeme route="reglages/securite" />
    {:else if (page === 'energie' && !route.startsWith('energie/solaire')) || route.startsWith('reglages/energie')}
      <!-- `reglages/energie`: where it used to be (old links). -->
      <Energie />
    {:else if page === 'energie' && route.startsWith('energie/solaire')}
      <!-- The solar demo (a simulation): loaded only when visited. -->
      {#await import('../../pages/Solaire.svelte') then { default: Solaire }}<Solaire />{/await}
    {:else if page === 'impression'}
      <Impression />
    {:else if page === 'dehors'}
      <Dehors />
    {:else if page === 'moli'}
      <Moli />
    {:else if page === 'automatismes'}
      <Automatismes {route} />
    {:else if page === 'systeme' || page === 'reglages'}
      <Systeme {route} />
    {:else if page === 'infra'}
      <!-- The house's machines: loaded only when visited. -->
      {#await import('../../pages/Infra.svelte') then { default: Infra }}<Infra />{/await}
    {:else}
      <Accueil onask={() => (moliOpen = true)} />
    {/if}
    </div>
    </SkyHero>
  </main>

  <Dock {page} moli={page !== 'moli' && !isHome} busy={moli.busy} onask={() => (moliOpen = true)} />

  {#if moliOpen}
    <MoliSheet onclose={() => (moliOpen = false)} />
  {/if}

  {#if alerts.open}
    {#key alerts.opened}<AlertWizard />{/key}
  {/if}

  {#if home.held}
    <PinSheet />
  {/if}

  {#if hub.session?.login_required}
    <!-- From the Internet, not signed in: nothing of the house, only this. -->
    <Login />
  {/if}

  {#if hub.session?.setup && !home.setupLater}
    <Setup onlater={() => (home.setupLater = true)} />
  {/if}

  {#if offline}
    <div class="offline" role="status"><Icon name="offline" size={16} />{t('commun.hors_ligne')}</div>
  {/if}

  <div class="notices" role="status" aria-live="polite">
    {#each hub.notices as n (n.id)}
      <div class="notice">
        <span class="bell"><Icon name="bell" size={18} /></span>
        <div>
          <b>{n.title ?? n.from ?? 'Moli'}</b>
          <p>{n.message}</p>
          {#if n.title && n.from}<small>{n.from}</small>{/if}
        </div>
        <button onclick={() => (hub.notices = hub.notices.filter((x) => x.id !== n.id))} aria-label={t('commun.fermer')}><Icon name="close" size={16} /></button>
      </div>
    {/each}
  </div>

  <div class="notes" role="status" aria-live="polite">
    {#each home.notes as n (n.id)}
      <p class="note {n.tone}">{n.text}</p>
    {/each}
  </div>
</div>

<style>
  .content {
    min-width: 0;
  }

  /* Every page in the home page's column, above the dock. */
  .page {
    flex: 1;
    display: flex;
    flex-direction: column;
    width: 100%;
    max-width: 1320px;
    margin: 0 auto;
    padding: clamp(18px, 4vh, 40px) clamp(20px, 5vw, 72px) 110px;
  }

  .page > :global(*) {
    min-width: 0;
  }

  .page.bare {
    max-width: none;
    padding: 0;
  }

  .notes {
    position: fixed;
    left: 50%;
    bottom: 92px;
    transform: translateX(-50%);
    display: grid;
    gap: 8px;
    z-index: 40;
    pointer-events: none;
  }

  /* The link to Moli dropped (a restart, the Wi-Fi): say so instead of a
     dashboard that silently stops moving. */
  .offline {
    position: fixed;
    top: 12px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 60;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 14px;
    border-radius: 999px;
    background: var(--warm);
    color: #1c1407;
    font-size: 13.5px;
    font-weight: 600;
    box-shadow: var(--shadow);
  }

  .notices {
    position: fixed;
    top: 18px;
    right: 18px;
    z-index: 42;
    display: grid;
    gap: 10px;
    width: min(380px, calc(100vw - 32px));
  }

  .notice {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    padding: 14px 12px 14px 14px;
    border-radius: var(--r-md);
    background: var(--surface);
    box-shadow: var(--shadow-lift), inset 0 0 0 1px var(--line);
    animation: drop 0.4s var(--ease) both;
  }

  .notice .bell {
    width: 36px;
    height: 36px;
    flex: none;
    border-radius: 12px;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, #b05fd8 14%, var(--surface));
    color: #b05fd8;
  }

  .notice div {
    flex: 1;
    display: grid;
    gap: 2px;
  }

  .notice p {
    font-size: 14.5px;
    line-height: 1.4;
  }

  .notice small {
    color: var(--ink-3);
    font-size: 12px;
  }

  .notice button {
    border: 0;
    background: none;
    color: var(--ink-3);
    padding: 2px;
  }

  @keyframes drop {
    from {
      opacity: 0;
      transform: translateY(-10px);
    }
  }

  .note {
    background: var(--ink);
    color: var(--on-ink);
    padding: 12px 18px;
    border-radius: 16px;
    font-weight: 550;
    box-shadow: var(--shadow-lift);
  }

  .note.error {
    background: var(--alert);
    color: #fff;
  }

  @media (max-width: 760px) {
    .page {
      padding-bottom: 130px;
    }

    .notes {
      width: calc(100% - 32px);
    }
  }
</style>

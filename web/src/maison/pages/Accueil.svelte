<script>
  // The home page: a sentence over the sky, what is going on, and the house by
  // use or room by room. The orders wait in a panel until someone asks for them.
  import { onMount } from 'svelte';
  import { home, num, greeting, isOn, value, nameOf } from '../lib/home.svelte.js';
  import { accueil, setView, activity, alarmArmed, alarmWord, alarms, chargers, chargerWord, clims, covers, coverOf, coversWord, garageOpen, garageWord, garages, indoorTemp, lights, notices, rooms, smokes, speakers, cameras } from '../lib/accueil.svelte.js';
  import { arranged } from '../lib/order.svelte.js';
  import { energy, useEnergy, kwh } from '../lib/energy-live.svelte.js';
  import { moli } from '../lib/moli.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from '../ui/Icon.svelte';
  import HomePanel from '../ui/HomePanel.svelte';
  import RoomTile from '../ui/RoomTile.svelte';
  import Sortable from '../ui/Sortable.svelte';
  import Personalize from '../ui/Personalize.svelte';
  import HomeCover from '../ui/HomeCover.svelte';
  import Orb from '../moli/Orb.svelte';
  import { appleTv } from '../lib/appletv.svelte.js';

  let { onask } = $props();

  onMount(useEnergy);

  let open = $state(null);

  const said = $derived(notices());
  /** The sentence: only what really asks for attention. */
  const urgent = $derived(said.find((n) => n.tone === 'alert') ?? said.find((n) => n.tone === 'warm'));
  const chips = $derived(said.filter((n) => n !== urgent));
  const going = $derived(activity());

  const lit = $derived(lights().filter((d) => isOn(d.id)).length);
  const inside = $derived(indoorTemp());
  const climOn = $derived(clims().filter((d) => value(d.id, 'on') === true).length);
  const playing = $derived(speakers().filter((d) => value(d.id, 'playing') === true));

  /** A door only for what the house has. */
  const doors = $derived.by(() => {
    const list = [];
    if (lights().length) list.push({ key: 'lights', icon: 'light', value: lit ? t('maison.accueil.lumieres_allumees', { count: lit }) : t('maison.accueil.rien_allume'), active: lit > 0 });
    if (covers().length) list.push({ key: 'covers', icon: 'shutter', value: coversWord(), active: covers().some((d) => coverOf(d).open) });
    if (clims().length || inside != null) list.push({ key: 'climate', icon: 'air-conditioner', value: inside != null ? t('maison.accueil.dedans', { v: num(inside, 1) }) : t('maison.accueil.usage.climate'), sub: clims().length ? (climOn ? t('maison.accueil.clims_marche', { count: climOn }) : t('maison.accueil.clims_eteintes')) : null, active: climOn > 0 });
    const atv = appleTv();
    if (atv) {
      const on = value(atv.id, 'power') === true;
      const state = value(atv.id, 'media_state');
      list.push({
        key: 'tv',
        icon: 'tv',
        value: on ? (state === 'playing' ? t('maison.accueil.tele_lecture') : t('maison.accueil.tele_allumee')) : t('maison.accueil.tele_eteinte'),
        sub: on ? value(atv.id, 'audio_output') : null,
        active: on,
      });
    }
    if (speakers().length) list.push({ key: 'music', icon: 'speaker', value: playing.length ? playing.map((d) => nameOf(d.id)).join(', ') : t('maison.accueil.silence'), sub: playing.length ? value(playing[0].id, 'title') : null, active: playing.length > 0 });
    const alarm = alarms()[0];
    const garage = garages()[0];
    if (alarm || garage || smokes().length || cameras().length) {
      list.push({
        key: 'security',
        icon: alarm && alarmArmed(alarm) ? 'shield-lock' : 'shield',
        value: alarm ? t('maison.accueil.alarme_etat', { etat: alarmWord(alarm).toLowerCase() }) : t('maison.accueil.rien_a_signaler'),
        sub: garage ? `${nameOf(garage.id)} : ${garageWord(garage).toLowerCase()}` : null,
        alert: !!garage && garageOpen(garage),
      });
    }
    const car = chargers()[0];
    if (car) list.push({ key: 'car', icon: 'car', value: chargerWord(car), active: value(car.id, 'work_state') === 'WORKING' });
    if (!energy.missing && energy.summary) list.push({ key: 'energy', icon: 'bolt', value: kwh(energy.summary.today?.total?.kwh) });
    return list;
  });

  const houseRooms = $derived(rooms());
  let editing = $state(false);
  const LISTS = ['accueil-usages', 'accueil-pieces'];
  // As many columns as doors shown.
  const shownDoors = $derived(arranged('accueil-usages', doors, (d) => d.key, editing).length);
  const close = () => (open = null);
  const follow = (item) => {
    if (item.panel) open = item.panel;
  };
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && close()} />

<div class="home">
    <section class="lead">
      <h1 class="home-h1">
        {greeting(home.now)}.<br />
        {#if urgent}<em class="warn">{urgent.text}.</em>{:else}<em>{t('maison.accueil.calme_court')}</em>{/if}
      </h1>

      {#if urgent}
        {#if urgent.href}
          <a class="fix" href={urgent.href}>{t('maison.accueil.voir')}<Icon name="chevron-right" size={16} /></a>
        {:else}
          <button class="fix" onclick={() => follow(urgent)}>{t('maison.accueil.voir')}<Icon name="chevron-right" size={16} /></button>
        {/if}
      {/if}

      {#if going.length || chips.length}
        <ul class="now" aria-label={t('maison.accueil.en_ce_moment')}>
          {#each chips as n (n.text)}
            <li><a class="pill {n.tone}" href={n.href ?? '#/'}><span class="dot"><Icon name={n.icon} size={15} /></span><span class="txt"><b>{n.text}</b></span></a></li>
          {/each}
          {#each going as item (item.key)}
            <li>
              {#snippet inner()}
                {#if item.cover}<HomeCover id={item.cover.id} art={item.cover.art} size={36} />{:else}<span class="dot"><Icon name={item.icon} size={15} /></span>{/if}
                <span class="txt"><b>{item.title}</b><small>{item.detail}</small></span>
              {/snippet}
              {#if item.panel}
                <button class="pill" onclick={() => follow(item)}>{@render inner()}</button>
              {:else}
                <a class="pill" href={item.href}>{@render inner()}</a>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <section class="house">
      <div class="tools">
        <div class="views" role="group" aria-label={t('maison.accueil.voir_maison')}>
          <button class:on={accueil.view === 'usages'} aria-pressed={accueil.view === 'usages'} onclick={() => setView('usages')}>{t('maison.accueil.par_usage')}</button>
          <button class:on={accueil.view === 'pieces'} aria-pressed={accueil.view === 'pieces'} onclick={() => setView('pieces')}>{t('maison.accueil.par_piece')}</button>
        </div>
        <Personalize bind:editing lists={LISTS} />
      </div>

      {#if accueil.view === 'usages'}
        {#snippet door(d)}
          <button class="door" class:active={d.active} class:alert={d.alert} onclick={() => (open = d.key)}>
            <span class="ico"><Icon name={d.icon} size={22} /></span>
            <span class="label">{t(`maison.accueil.usage.${d.key}`)}</span>
            <span class="value">{d.value}</span>
            {#if d.sub}<span class="sub">{d.sub}</span>{/if}
          </button>
        {/snippet}
        <nav class="doors" style:--n={shownDoors} aria-label={t('maison.accueil.par_usage')}>
          <Sortable list="accueil-usages" items={doors} id={(d) => d.key} label={(d) => t(`maison.accueil.usage.${d.key}`)} {editing} item={door} />
        </nav>
      {:else}
        {#snippet roomTile(r)}<RoomTile {r} onclick={() => (open = `room:${r.name}`)} />{/snippet}
        <nav class="rooms" aria-label={t('maison.accueil.par_piece')}>
          <Sortable list="accueil-pieces" items={houseRooms} id={(r) => r.name} label={(r) => r.name} {editing} item={roomTile} />
        </nav>
      {/if}
    </section>

    <footer>
      <button class="ask" onclick={onask}>
        <Orb size={28} state={moli.busy ? 'thinking' : 'idle'} />{t('commun.demander_moli')}…
      </button>
    </footer>
</div>

{#if open}
  <button class="scrim" aria-label={t('commun.fermer')} onclick={close}></button>
  <HomePanel panel={open} onclose={close} />
{/if}

<style>
  .home {
    flex: 1;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    gap: clamp(24px, 4.5vh, 48px);
    width: 100%;
    max-width: 1320px;
    margin: 0 auto;
    padding: clamp(28px, 7vh, 90px) clamp(20px, 5vw, 72px) 92px;
    color: #fff;
  }

  h1 {
    margin: 0;
    font-family: var(--serif);
    font-weight: 340;
    font-size: clamp(44px, 7.2vw, 96px);
    line-height: 1.03;
    letter-spacing: -0.025em;
    text-shadow: 0 2px 26px rgb(0 0 0 / 30%);
  }

  h1 em {
    font-style: italic;
    color: rgb(255 255 255 / 80%);
  }

  h1 em.warn {
    color: #ffd2c6;
    font-size: 0.72em;
  }

  .fix {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-top: 14px;
    padding: 9px 15px;
    border: 0;
    border-radius: 999px;
    background: var(--alert);
    color: #fff;
    font: inherit;
    font-weight: 650;
    text-decoration: none;
    cursor: pointer;
  }

  .now {
    list-style: none;
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin: 24px 0 0;
    padding: 0;
  }

  .pill,
  .door,
  .ask,
  .views {
    --glass: rgb(255 255 255 / 15%);
    --glass-2: rgb(255 255 255 / 24%);
    background: var(--glass);
    backdrop-filter: blur(16px);
    color: #fff;
  }

  .pill {
    display: flex;
    align-items: center;
    gap: 10px;
    max-width: min(420px, 88vw);
    padding: 6px 15px 6px 6px;
    border: 0;
    border-radius: 999px;
    font: inherit;
    text-align: left;
    text-decoration: none;
    cursor: pointer;
  }

  .pill:hover,
  .door:hover,
  .ask:hover {
    background: var(--glass-2);
  }

  .pill :global(img) {
    border-radius: 50%;
  }

  .dot {
    width: 36px;
    height: 36px;
    flex: none;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: rgb(255 197 92 / 28%);
    color: #ffd98e;
  }

  .txt {
    display: grid;
    min-width: 0;
    line-height: 1.22;
  }

  .txt b {
    font-size: 14px;
    font-weight: 620;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .txt small {
    color: rgb(255 255 255 / 72%);
    font-size: 12.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .house {
    display: grid;
    gap: 12px;
  }

  .views {
    display: inline-flex;
    gap: 2px;
    padding: 3px;
    border-radius: 999px;
  }

  .views button {
    padding: 7px 15px;
    border: 0;
    border-radius: 999px;
    background: none;
    color: rgb(255 255 255 / 80%);
    font: inherit;
    font-size: 13.5px;
    font-weight: 600;
    cursor: pointer;
  }

  .views button.on {
    background: rgb(255 255 255 / 26%);
    color: #fff;
  }

  .tools {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .doors {
    --sort-cols: repeat(var(--n, 6), minmax(0, 1fr));
    --sort-gap: 10px;
  }

  .rooms {
    --sort-cols: repeat(auto-fill, minmax(150px, 1fr));
    --sort-gap: 8px;
  }

  .door {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    min-height: 152px;
    padding: 18px;
    border: 0;
    border-radius: 22px;
    font: inherit;
    text-align: left;
    cursor: pointer;
    transition: background 0.2s, transform 0.2s var(--ease);
  }

  .door:hover {
    transform: translateY(-2px);
  }

  .door .ico {
    margin-bottom: 18px;
    color: rgb(255 255 255 / 76%);
  }

  .door .label {
    color: rgb(255 255 255 / 74%);
    font-size: 13px;
    font-weight: 550;
  }

  .door .value {
    font-size: 17px;
    font-weight: 640;
    line-height: 1.2;
  }

  .door .sub {
    max-width: 100%;
    color: rgb(255 255 255 / 74%);
    font-size: 13px;
    line-height: 1.25;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .door.active {
    background: rgb(255 190 92 / 26%);
  }

  .door.active .ico {
    color: #ffd98e;
  }

  .door.alert .ico,
  .door.alert .sub {
    color: #ffc1b1;
  }

  .rooms {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 8px;
  }

  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 14px 22px;
    flex-wrap: wrap;
  }

  .ask {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: min(380px, 100%);
    padding: 7px 18px 7px 8px;
    border: 0;
    border-radius: 999px;
    color: rgb(255 255 255 / 88%);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .scrim {
    position: fixed;
    inset: 0;
    z-index: 30;
    border: 0;
    background: rgb(5 8 18 / 42%);
    animation: fade 0.2s;
    cursor: default;
  }

  @keyframes fade {
    from {
      opacity: 0;
    }
  }

  @media (max-width: 1100px) {
    .doors {
      --sort-cols: repeat(3, minmax(0, 1fr));
    }

    .door {
      min-height: 128px;
    }

    .door .ico {
      margin-bottom: 10px;
    }
  }

  @media (max-width: 760px) {
    .doors,
    .rooms {
      --sort-cols: repeat(2, minmax(0, 1fr));
    }

    .door {
      min-height: 118px;
      padding: 15px;
      border-radius: 20px;
    }

    .ask {
      min-width: 100%;
    }

  }
</style>

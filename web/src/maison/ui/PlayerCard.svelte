<script>
  import { value, since, act, num, nameOf, reachable } from '../lib/home.svelte.js';
  import { has, list, steerable, mmss } from '../lib/speakers.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** A speaker card: what plays in a room (or a group of rooms), its buttons
   *  and volumes. `members`: the rooms, the one that leads first. `rooms`: all
   *  the rooms (to group with). `look`: what to show (`sections`, `ambient`). */
  let { members, rooms = [], look, groupKey } = $props();

  const lead = $derived(members[0].id);
  const src = $derived(value(lead, 'source'));
  const playing = $derived(value(lead, 'playing') === true);
  const title = $derived(value(lead, 'title'));
  const cover = $derived(value(lead, 'art'));
  const duration = $derived(value(lead, 'duration'));
  const repeat = $derived(value(lead, 'repeat') ?? 'off');
  const shuffle = $derived(value(lead, 'shuffle') === true);
  const lastOne = $derived(src === 'music' && typeof value(lead, 'queue_length') === 'number' && value(lead, 'queue_length') <= 1);
  const others = $derived(rooms.filter((r) => !members.some((m) => m.id === r.id)));
  const up = $derived(reachable(lead));
  const shown = $derived(cover && (failed[cover] ?? 0) < 2 && title);
  const ambient = $derived(shown && look.ambient);

  // The progress bar moves every second, not every time the house speaks.
  let now = $state(Date.now());
  $effect(() => {
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });
  // The position was told at some moment: while it plays, it moves on.
  const pos = $derived.by(() => {
    const p = value(lead, 'position');
    if (typeof p !== 'number' || !duration) return null;
    const told = since(lead, 'position') ?? now;
    const moved = playing ? Math.max(0, (now - told) / 1000) : 0;
    return Math.min(duration, p + moved);
  });

  let panel = $state({});
  const toggle = (key, which) => (panel[key] = panel[key] === which ? null : which);
  let dragging = $state({});
  const STEP = 5;
  const NEXT_REPEAT = { off: 'all', all: 'one', one: 'off' };
  const stateLabel = (id) =>
    value(id, 'playing') === true
      ? t('salon.enceinte.lecture')
      : value(id, 'state') === 'paused_playback'
        ? t('salon.enceinte.en_pause')
        : t('salon.enceinte.arret');
  // A cover comes through Moli (a browser away from home cannot reach a
  // speaker); a hub that does not know that yet is asked for nothing twice:
  // the speaker's own address, then no picture. Told apart by the address.
  const failed = $state({});
  const coverSrc = (id, art) => (failed[art] === 1 ? art : `/api/devices/${id}/cover?v=${encodeURIComponent(art)}`);
  const coverFailed = (art) => (failed[art] = (failed[art] ?? 0) + 1);
  const stepVolume = (id, delta) => {
    const v = value(id, 'volume');
    if (typeof v !== 'number') return;
    act(`${id}/volume`, Math.max(0, Math.min(100, Math.round(v) + delta)), t('salon.volume'));
  };
</script>

<div class="player">
  <section class="card group" class:lit={playing} class:down={!up} class:art={ambient} class:playing>
    {#if ambient}
      {#key cover}
        <div class="ambient" aria-hidden="true" style:background-image="url({coverSrc(lead, cover)})"></div>
      {/key}
    {/if}
    <div class="card-head">
      <h2><Icon name={members.length > 1 ? 'speakers' : 'speaker'} size={18} />{members.map((m) => nameOf(m.id)).join(' + ')}</h2>
<span class="chip {playing ? 'warm' : ''}">{up ? stateLabel(lead) : t('enceintes.injoignable')}</span>
    </div>

    <div class="now">
      {#if shown}
        {#key cover}
          <div class="cover">
            <img class="glow" src={coverSrc(lead, cover)} alt="" aria-hidden="true" />
            <img class="face" src={coverSrc(lead, cover)} alt="" referrerpolicy="no-referrer" onerror={() => coverFailed(cover)} />
          </div>
        {/key}
      {:else}
        <div class="noart"><Icon name="speaker" size={34} /></div>
      {/if}
      <div class="what">
        {#if src && src !== 'none'}
          <span class="eyebrow">
            {#if playing}<span class="eq" aria-hidden="true"><i></i><i></i><i></i></span>{/if}
            {t(`enceintes.source.${src}`)}
          </span>
        {/if}
        {#if title}
          <h3>{title}</h3>
          {#if value(lead, 'artist')}<p class="sub">{value(lead, 'artist')}{value(lead, 'album') ? ` · ${value(lead, 'album')}` : ''}</p>{/if}
        {:else if playing}
          <!-- Playing, the speaker has not said what yet: not « nothing ». -->
          <h3>{t('enceintes.lecture_en_cours')}</h3>
        {:else}
          <h3 class="muted">{t('salon.enceinte.rien')}</h3>
        {/if}
        {#if look.sections.progress && duration && pos != null}
          <div class="progress" aria-hidden="true"><span style:width="{Math.min(100, (pos / duration) * 100)}%"></span></div>
          <p class="times num"><span>{mmss(pos)}</span><span>{mmss(duration)}</span></p>
        {/if}
      </div>
    </div>

    <div class="transport">
      {#if has(lead, 'shuffle')}
        <button class="soft" class:active={shuffle} disabled={!steerable(src)} onclick={() => act(`${lead}/shuffle`, !shuffle, t('enceintes.aleatoire'))} aria-label={t('enceintes.aleatoire')} aria-pressed={shuffle}>
          <Icon name="shuffle" size={20} />
        </button>
      {/if}
      {#if has(lead, 'skip')}
        <button class="soft" disabled={!steerable(src) || lastOne} onclick={() => act(`${lead}/skip`, 'previous', t('enceintes.precedent'))} aria-label={t('enceintes.precedent')}>
          <Icon name="previous" size={24} />
        </button>
      {/if}
      <button class="round" onclick={() => act(`${lead}/playing`, !playing, t('salon.lecture'))} aria-label={playing ? t('salon.pause') : t('salon.lecture')}>
        <Icon name={playing ? 'pause' : 'play'} size={28} />
      </button>
      {#if has(lead, 'skip')}
        <button class="soft" disabled={!steerable(src) || lastOne} onclick={() => act(`${lead}/skip`, 'next', t('enceintes.suivant'))} aria-label={t('enceintes.suivant')}>
          <Icon name="next" size={24} />
        </button>
      {/if}
      {#if has(lead, 'repeat')}
        <button class="soft" class:active={repeat !== 'off'} disabled={!steerable(src)} onclick={() => act(`${lead}/repeat`, NEXT_REPEAT[repeat] ?? 'off', t('enceintes.repetition.titre'))} aria-label={t(`enceintes.repetition.${repeat}`)}>
          <Icon name={repeat === 'one' ? 'repeat-once' : 'repeat'} size={20} />
        </button>
      {/if}
    </div>

    {#if look.sections.volumes}
    <ul class="members">
      {#each members as m (m.id)}
        {@const volume = dragging[m.id] ?? value(m.id, 'volume')}
        {@const muted = value(m.id, 'mute') === true}
        <li class:down={!reachable(m.id)}>
          <div class="who">
            <b>{nameOf(m.id)}</b>
            {#if m.description}<span class="chip">{m.description}</span>{/if}
            {#if !reachable(m.id)}<span class="chip alert">{t('enceintes.injoignable')}</span>{/if}
          </div>
          <div class="vol">
            <button class="soft small" class:active={muted} onclick={() => act(`${m.id}/mute`, !muted, t('salon.sourdine'))} aria-label={t('salon.couper_son')}>
              <Icon name={muted ? 'volume-off' : 'volume-high'} size={18} />
            </button>
            <button class="soft small" disabled={!reachable(m.id) || volume == null} onclick={() => stepVolume(m.id, -STEP)} aria-label={t('enceintes.baisser', { room: nameOf(m.id) })} title={t('enceintes.baisser', { room: nameOf(m.id) })}>
              <Icon name="volume-minus" size={18} />
            </button>
            <input
              type="range"
              min="0"
              max="100"
              value={volume ?? 0}
              disabled={!reachable(m.id)}
              oninput={(e) => (dragging[m.id] = Number(e.currentTarget.value))}
              onchange={(e) => {
                const v = Number(e.currentTarget.value);
                delete dragging[m.id];
                act(`${m.id}/volume`, v, t('salon.volume'));
              }}
              aria-label={t('enceintes.volume_de', { room: nameOf(m.id) })} />
            <button class="soft small" disabled={!reachable(m.id) || volume == null} onclick={() => stepVolume(m.id, STEP)} aria-label={t('enceintes.monter', { room: nameOf(m.id) })} title={t('enceintes.monter', { room: nameOf(m.id) })}>
              <Icon name="volume-plus" size={18} />
            </button>
            <b class="num">{num(volume)}</b>
          </div>
          {#if members.length > 1 && has(m.id, 'leave')}
            <button class="soft small" onclick={() => act(`${m.id}/leave`, true, t('enceintes.quitter'))} title={t('enceintes.quitter')} aria-label={t('enceintes.quitter_de', { room: nameOf(m.id) })}>
              <Icon name="link-off" size={18} />
            </button>
          {/if}
        </li>
      {/each}
    </ul>
    {/if}

    {#if look.sections.join && others.length && has(others[0].id, 'join')}
      <div class="join">
        <span class="muted small">{t('enceintes.ajouter')}</span>
        {#each others as o (o.id)}
          <button class="chip pick" disabled={!reachable(o.id)} onclick={() => act(`${o.id}/join`, groupKey, t('enceintes.ajouter'))}>
            <Icon name="plus" size={14} />{nameOf(o.id)}
          </button>
        {/each}
      </div>
    {/if}

    {#if look.sections.library && has(lead, 'favorite')}
      <div class="tabs" role="group">
        <button class="chip" class:cool={panel[groupKey] === 'favorites'} onclick={() => toggle(groupKey, 'favorites')}><Icon name="heart" size={14} />{t('enceintes.favoris')}</button>
        <button class="chip" class:cool={panel[groupKey] === 'queue'} onclick={() => toggle(groupKey, 'queue')}><Icon name="playlist" size={14} />{t('enceintes.file')}{value(lead, 'queue_length') ? ` · ${value(lead, 'queue_length')}` : ''}</button>
      </div>
      {#if panel[groupKey] === 'favorites'}
        {@const favorites = list(lead, 'favorites')}
        <ul class="lib">
          {#each favorites as name, i (i)}
            <li><button onclick={() => act(`${lead}/favorite`, name, t('enceintes.lancer', { name }))}>{name}</button></li>
          {:else}
            <li class="muted">{t('enceintes.aucun_favori')}</li>
          {/each}
        </ul>
      {:else if panel[groupKey] === 'queue'}
        {@const queue = list(lead, 'queue')}
        <ul class="lib">
          {#each queue as item, i (i)}
            <li class:current={value(lead, 'queue_position') === i + 1}>
              <button onclick={() => act(`${lead}/queue_play`, i + 1, t('enceintes.lancer', { name: item.title }))}>
                <span class="num n">{i + 1}</span><span class="t">{item.title}{#if item.artist}<span class="muted"> · {item.artist}</span>{/if}</span>
              </button>
            </li>
          {:else}
            <li class="muted">{t('enceintes.file_vide')}</li>
          {/each}
        </ul>
        {#if queue.length && has(lead, 'queue_clear')}
          <button class="chip clear" onclick={() => act(`${lead}/queue_clear`, true, t('enceintes.vider'))}><Icon name="trash" size={14} />{t('enceintes.vider')}</button>
        {/if}
      {/if}
    {/if}
  </section>
</div>

<style>


  /* The cover, the title and the buttons grow with the tile (about 16:9). */
  .player .group {
    --cover: clamp(72px, 21cqw, 220px);
    --title: clamp(15px, 3.4cqw, 32px);
    --round: clamp(42px, 8.5cqw, 84px);
    --soft: clamp(34px, 6.5cqw, 62px);
    padding: clamp(14px, 3cqw, 28px);
    min-height: 40cqw;
    display: grid;
    grid-template-columns: var(--cover) minmax(0, 1fr);
    column-gap: clamp(12px, 3cqw, 28px);
    align-content: start;
  }

  .group > :global(*) {
    grid-column: 1 / -1;
  }

  .group .card-head {
    margin-bottom: clamp(8px, 2cqw, 16px);
  }

  .group .now {
    display: contents;
  }

  .group .cover,
  .group .noart {
    grid-column: 1;
    grid-row: 2 / span 2;
    align-self: center;
  }

  .group .what {
    grid-column: 2;
    grid-row: 2;
    align-self: end;
  }

  .group .transport {
    grid-column: 2;
    grid-row: 3;
    justify-content: flex-start;
    align-self: start;
    margin: 10px 0 4px;
    gap: clamp(8px, 1.6cqw, 16px);
  }

  .group .members li {
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 8px;
    padding: 8px 12px;
  }

  .group .members li .vol {
    grid-column: 1 / -1;
    order: 3;
  }

  .group.down {
    opacity: 0.7;
  }

  .now {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 20px;
    align-items: center;
  }

  .noart {
    width: var(--cover);
    height: var(--cover);
    border-radius: var(--r-sm);
    background: var(--surface-3);
    display: grid;
    place-items: center;
    color: var(--ink-3);
  }

  .what {
    min-width: 0;
    display: grid;
    gap: 2px;
  }

  .what h3 {
    font-size: var(--title);
    line-height: 1.15;
    font-weight: 650;
    overflow-wrap: anywhere;
  }

  .eyebrow {
    font-size: 12px;
    font-weight: 650;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--warm-ink);
  }

  .small {
    font-size: 13px;
  }

  .progress {
    height: 6px;
    border-radius: 99px;
    background: var(--surface-3);
    margin-top: 12px;
    overflow: hidden;
  }

  .progress span {
    display: block;
    height: 100%;
    background: var(--warm);
    border-radius: 99px;
    box-shadow: 0 0 12px var(--warm-glow);
    transition: width 1s linear;
  }

  .times {
    display: flex;
    justify-content: space-between;
    font-size: 12px;
    color: var(--ink-3);
    margin-top: 4px;
  }

  .sub {
    color: var(--ink-2);
    overflow-wrap: anywhere;
  }

  .eq {
    display: inline-flex;
    align-items: flex-end;
    gap: 2px;
    height: 12px;
    margin-right: 6px;
    vertical-align: -1px;
  }

  .eq i {
    width: 3px;
    height: 3px;
    border-radius: 2px;
    background: currentColor;
    animation: eq 0.9s ease-in-out infinite;
  }

  .eq i:nth-child(2) {
    animation-delay: 0.25s;
  }

  .eq i:nth-child(3) {
    animation-delay: 0.5s;
  }

  @keyframes eq {
    0%,
    100% {
      height: 3px;
    }
    50% {
      height: 12px;
    }
  }

  /* The cover, big, and its own colours behind it: the card becomes the
     player of what plays. */
  .cover {
    position: relative;
    width: var(--cover);
    height: var(--cover);
  }

  .cover img {
    width: 100%;
    height: 100%;
    border-radius: calc(var(--cover) * 0.15);
    object-fit: cover;
    display: block;
  }

  /* The halo: as big as the cover (not its picture's own size: that one
     covered the buttons), and never in the way of a finger. */
  .cover .glow {
    position: absolute;
    inset: 10px;
    z-index: 0;
    width: calc(100% - 20px);
    height: calc(100% - 20px);
    filter: blur(24px) saturate(1.8);
    opacity: 0.75;
    pointer-events: none;
  }

  .cover .face {
    position: relative;
    z-index: 1;
    box-shadow: 0 18px 40px -12px rgb(0 0 0 / 65%);
    animation: pop 0.6s var(--ease);
  }

  .playing .cover .face {
    animation:
      pop 0.6s var(--ease),
      breathe 6s ease-in-out 0.6s infinite;
  }

  @keyframes pop {
    from {
      opacity: 0;
      transform: scale(0.92);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }

  @keyframes breathe {
    50% {
      transform: scale(1.025);
    }
  }

  .group.art {
    overflow: hidden;
    isolation: isolate;
    background: #10131a;
    color: #fff;
    --ink: #fff;
    --ink-2: rgb(255 255 255 / 84%);
    --ink-3: rgb(255 255 255 / 64%);
    --bg: #10131a;
    --surface-2: rgb(255 255 255 / 12%);
    --surface-3: rgb(255 255 255 / 22%);
    --warm-soft: rgb(255 255 255 / 24%);
    --warm-ink: #fff;
    --warm-glow: rgb(255 255 255 / 55%);
    --warm: #fff;
  }

  .ambient {
    pointer-events: none;
    position: absolute;
    inset: -30%;
    z-index: -2;
    background-size: cover;
    background-position: center;
    filter: blur(50px) saturate(1.8) brightness(0.55);
    animation: fade 0.9s var(--ease);
  }

  .group.art::after {
    pointer-events: none;
    content: '';
    position: absolute;
    inset: 0;
    z-index: -1;
    border-radius: inherit;
    background: linear-gradient(180deg, rgb(8 10 16 / 52%), rgb(8 10 16 / 86%));
  }

  @keyframes fade {
    from {
      opacity: 0;
    }
  }

  .group.art .chip,
  .group.art .eyebrow,
  .group.art h3 {
    color: #fff;
  }

  .group.art .what h3 {
    text-shadow: 0 2px 14px rgb(0 0 0 / 45%);
  }

  .group.art .eyebrow {
    opacity: 0.9;
  }

  .group.art button.round {
    box-shadow: 0 10px 28px -8px rgb(0 0 0 / 60%);
  }

  .group.art .members li {
    backdrop-filter: blur(10px);
  }

  @media (prefers-reduced-motion: reduce) {
    .eq i,
    .cover .face,
    .playing .cover .face,
    .ambient {
      animation: none;
    }
  }

  .transport {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 14px;
    margin: 18px 0 8px;
  }

  button.round,
  button.soft {
    border: 0;
    border-radius: 50%;
    display: grid;
    place-items: center;
  }

  button.round {
    width: var(--round);
    height: var(--round);
    background: var(--ink);
    color: var(--bg);
  }

  button.soft {
    width: var(--soft);
    height: var(--soft);
    background: var(--surface-2);
    color: var(--ink-2);
  }

  button.soft.small {
    width: 36px;
    height: 36px;
  }

  button.soft.active {
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  button.soft:disabled {
    opacity: 0.4;
  }

  .members {
    list-style: none;
    margin: 12px 0 0;
    padding: 0;
    display: grid;
    gap: 10px;
  }

  .members li {
    display: grid;
    grid-template-columns: minmax(90px, 0.8fr) minmax(0, 1.6fr) auto;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border-radius: var(--r-sm);
    background: var(--surface-2);
  }

  .members li.down {
    opacity: 0.6;
  }

  .who {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }

  .vol {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .vol input {
    flex: 1;
    min-width: 0;
    accent-color: var(--warm);
  }

  .vol b {
    min-width: 2.2ch;
    text-align: right;
  }

  .join,
  .tabs {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-top: 14px;
  }

  button.chip {
    border: 0;
  }

  button.chip.pick:hover:not(:disabled),
  button.chip.clear:hover {
    background: var(--surface-3);
  }

  button.chip:disabled {
    opacity: 0.5;
  }

  .lib {
    list-style: none;
    margin: 10px 0 0;
    padding: 0;
    max-height: 320px;
    overflow: auto;
    display: grid;
    gap: 2px;
  }

  .lib li button {
    width: 100%;
    text-align: left;
    border: 0;
    background: none;
    padding: 9px 10px;
    border-radius: 10px;
    display: flex;
    gap: 10px;
  }

  .lib li button:hover {
    background: var(--surface-2);
  }

  .lib li.current button {
    background: var(--warm-soft);
    color: var(--warm-ink);
    font-weight: 600;
  }

  .lib .n {
    min-width: 2.4ch;
    color: var(--ink-3);
  }

  .lib .t {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .clear {
    margin-top: 10px;
  }

  @media (max-width: 520px) {
    .members li {
      grid-template-columns: minmax(0, 1fr) auto;
    }

    .members li .vol {
      grid-column: 1 / -1;
      order: 3;
    }
  }</style>

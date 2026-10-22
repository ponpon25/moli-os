<script>
  import { onMount } from 'svelte';
  import { value, since, act, num, nameOf, reachable } from '../lib/home.svelte.js';
  import { has, list, steerable, mmss } from '../lib/speakers.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';
  import Marquee from './Marquee.svelte';

  /** Full-screen player for one room or group: the cover, the lyrics, the
   *  buttons. `group`: what is shown; `groups`: all the groups (to switch). */
  let { group, groups = [], onpick, onclose } = $props();

  const members = $derived(group.members);
  const lead = $derived(members[0].id);
  const title = $derived(value(lead, 'title'));
  const artist = $derived(value(lead, 'artist'));
  const album = $derived(value(lead, 'album'));
  const cover = $derived(value(lead, 'art'));
  const src = $derived(value(lead, 'source'));
  const playing = $derived(value(lead, 'playing') === true);
  const duration = $derived(value(lead, 'duration'));
  const repeat = $derived(value(lead, 'repeat') ?? 'off');
  const shuffle = $derived(value(lead, 'shuffle') === true);
  const lastOne = $derived(src === 'music' && typeof value(lead, 'queue_length') === 'number' && value(lead, 'queue_length') <= 1);
  const NEXT_REPEAT = { off: 'all', all: 'one', one: 'off' };

  let coverFailed = $state({});
  const coverUrl = $derived(cover ? `/api/devices/${lead}/cover?v=${encodeURIComponent(cover)}` : null);
  const shown = $derived(coverUrl && !coverFailed[cover] && title);

  // The position was told at some moment: while it plays, it moves on.
  let now = $state(Date.now());
  const pos = $derived.by(() => {
    const p = value(lead, 'position');
    if (typeof p !== 'number' || !duration) return null;
    const told = since(lead, 'position') ?? now;
    return Math.min(duration, p + (playing ? Math.max(0, (now - told) / 1000) : 0));
  });

  // Lyrics, only for what Apple Music plays: asked once per song.
  const song = $derived(value(lead, 'apple_song'));
  const canLyrics = $derived(!!song && has(lead, 'lyrics'));
  let wantLyrics = $state(true);
  const showLyrics = $derived(canLyrics && wantLyrics);
  let asked = $state(null);
  let failed = $state(null);
  // The lyrics: a timed pause of the singer (or a long wait before the first
  // words) shows as three dots, like in Apple Music.
  const lyrics = $derived.by(() => {
    if (!song || value(lead, 'lyrics_for') !== song) return null;
    const raw = list(lead, 'lyrics_lines');
    if (raw.length && typeof raw[0].t === 'number' && raw[0].t > 4) return [{ t: 0, text: '' }, ...raw];
    return raw;
  });
  const timed = $derived(!!lyrics?.length && lyrics.every((l) => typeof l.t === 'number'));
  const current = $derived.by(() => {
    if (!timed || pos == null) return -1;
    let at = -1;
    for (let i = 0; i < lyrics.length; i++) if (lyrics[i].t <= pos + 0.4) at = i;
    return at;
  });
  $effect(() => {
    if (showLyrics && lyrics == null && asked !== song) {
      asked = song;
      act(`${lead}/lyrics`, JSON.stringify({ song, title, artist, album: album ?? '', duration }), t('enceintes.paroles.titre')).then((r) => {
        if (r !== 'ok') failed = song;
      });
    }
  });
  let lyricsBox = $state();
  $effect(() => {
    if (current < 0 || !lyricsBox) return;
    const line = lyricsBox.children[current];
    if (line) lyricsBox.scrollTo({ top: line.offsetTop - lyricsBox.clientHeight * 0.3, behavior: 'smooth' });
  });

  // Karaoke: the sung line fills as it goes. The source times lines, not
  // words, so the sweep is spread over the line (by its length, and never
  // longer than the line lasts) rather than measured word by word.
  const SPEED = 0.085;
  function sweep(i) {
    if (!timed || i !== current || pos == null) return 0;
    const line = lyrics[i];
    const next = lyrics[i + 1]?.t ?? line.t + 6;
    const span = Math.min(Math.max(next - line.t, 0.8), Math.max(1.2, line.text.length * SPEED));
    return Math.max(0, Math.min(1, (pos + 0.15 - line.t) / span)) * 100;
  }

  // Volumes: one slider per room, behind a button.
  let volumes = $state(false);
  let dragging = $state({});
  const STEP = 5;
  const stepVolume = (id, delta) => {
    const v = value(id, 'volume');
    if (typeof v === 'number') act(`${id}/volume`, Math.max(0, Math.min(100, Math.round(v) + delta)), t('salon.volume'));
  };

  // After a while without a touch, the buttons go away and only the music stays.
  const IDLE = 15000;
  let idle = $state(false);
  let timer;
  function wake() {
    idle = false;
    clearTimeout(timer);
    timer = setTimeout(() => (idle = !volumes), IDLE);
  }
  function key(e) {
    wake();
    if (e.target.closest?.('input, select')) return;
    if (e.key === 'Escape') onclose();
    else if (e.key === ' ') {
      e.preventDefault();
      act(`${lead}/playing`, !playing, t('salon.lecture'));
    } else if (e.key === 'ArrowRight' && has(lead, 'skip') && steerable(src)) act(`${lead}/skip`, 'next', t('enceintes.suivant'));
    else if (e.key === 'ArrowLeft' && has(lead, 'skip') && steerable(src)) act(`${lead}/skip`, 'previous', t('enceintes.precedent'));
  }
  onMount(() => {
    const tick = setInterval(() => (now = Date.now()), 100);
    wake();
    const previous = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => {
      clearInterval(tick);
      clearTimeout(timer);
      document.body.style.overflow = previous;
    };
  });
</script>

<svelte:window onkeydown={key} />

<div class="immersion" class:idle role="dialog" aria-modal="true" aria-label={t('enceintes.immersion.titre')} onpointermove={wake} onpointerdown={wake}>
  {#key cover}
    {#if shown}<div class="backdrop" aria-hidden="true" style:background-image="url({coverUrl})"></div>{/if}
  {/key}
  <div class="veil" aria-hidden="true"></div>

  <header class="top">
    <button class="soft" onclick={onclose} aria-label={t('enceintes.immersion.fermer')} title={t('enceintes.immersion.fermer')}><Icon name="close" size={22} /></button>
    <nav class="picks" aria-label={t('enceintes.immersion.choisir')}>
      {#each groups as g (g.key)}
        <button class="chip" class:on={g.key === group.key} aria-pressed={g.key === group.key} onclick={() => onpick(g.key)}>{g.members.map((m) => nameOf(m.id)).join(' + ')}</button>
      {/each}
    </nav>
  </header>

  <main class="stage" class:solo={!showLyrics}>
    <section class="deck">
      {#key cover}
        <div class="art">
          {#if shown}
            <img class="glow" src={coverUrl} alt="" aria-hidden="true" />
            <img class="face" class:breathe={playing} src={coverUrl} alt="" referrerpolicy="no-referrer" onerror={() => (coverFailed[cover] = true)} />
          {:else}
            <div class="noart"><Icon name="speaker" size={72} /></div>
          {/if}
        </div>
      {/key}
      <div class="what">
        {#if title}
          <h2><Marquee text={title} /></h2>
          {#if artist}<p class="sub"><Marquee text={album ? `${artist} · ${album}` : artist} /></p>{/if}
        {:else if playing}
          <h2>{t('enceintes.lecture_en_cours')}</h2>
        {:else}
          <h2 class="dim">{t('salon.enceinte.rien')}</h2>
        {/if}
      </div>
    </section>

    {#if showLyrics}
      <section class="words" bind:this={lyricsBox} aria-label={t('enceintes.paroles.titre')}>
        {#if lyrics == null && failed !== song}
          <p class="dim">{t('enceintes.paroles.chargement')}</p>
        {:else if lyrics == null}
          <p class="dim">{t('enceintes.paroles.aucune')}</p>
        {:else}
          {#each lyrics as line, i (i)}
            {#if line.text}
              <p class:now={i === current} class:near={timed && Math.abs(i - current) === 1} style:--p="{sweep(i)}%"><span class="k">{line.text}</span></p>
            {:else}
              <p class="pause" class:now={i === current} aria-hidden="true"><i></i><i></i><i></i></p>
            {/if}
          {:else}
            <p class="dim">{t('enceintes.paroles.aucune')}</p>
          {/each}
        {/if}
      </section>
    {/if}
  </main>

  <footer class="bottom">
    {#if duration && pos != null}
      <div class="bar">
        <span class="num">{mmss(pos)}</span>
        <div class="progress" aria-hidden="true"><span style:width="{Math.min(100, (pos / duration) * 100)}%"></span></div>
        <span class="num">{mmss(duration)}</span>
      </div>
    {/if}
    <div class="transport">
      {#if has(lead, 'shuffle')}
        <button class="soft" class:active={shuffle} disabled={!steerable(src)} onclick={() => act(`${lead}/shuffle`, !shuffle, t('enceintes.aleatoire'))} aria-label={t('enceintes.aleatoire')} aria-pressed={shuffle}><Icon name="shuffle" size={24} /></button>
      {/if}
      {#if has(lead, 'skip')}
        <button class="soft" disabled={!steerable(src) || lastOne} onclick={() => act(`${lead}/skip`, 'previous', t('enceintes.precedent'))} aria-label={t('enceintes.precedent')}><Icon name="previous" size={30} /></button>
      {/if}
      <button class="round" onclick={() => act(`${lead}/playing`, !playing, t('salon.lecture'))} aria-label={playing ? t('salon.pause') : t('salon.lecture')}><Icon name={playing ? 'pause' : 'play'} size={38} /></button>
      {#if has(lead, 'skip')}
        <button class="soft" disabled={!steerable(src) || lastOne} onclick={() => act(`${lead}/skip`, 'next', t('enceintes.suivant'))} aria-label={t('enceintes.suivant')}><Icon name="next" size={30} /></button>
      {/if}
      {#if has(lead, 'repeat')}
        <button class="soft" class:active={repeat !== 'off'} disabled={!steerable(src)} onclick={() => act(`${lead}/repeat`, NEXT_REPEAT[repeat] ?? 'off', t('enceintes.repetition.titre'))} aria-label={t(`enceintes.repetition.${repeat}`)}><Icon name={repeat === 'one' ? 'repeat-once' : 'repeat'} size={24} /></button>
      {/if}
      <span class="gap" aria-hidden="true"></span>
      {#if has(lead, 'lyrics')}
        <button class="soft" class:active={showLyrics} disabled={!canLyrics} onclick={() => (wantLyrics = !wantLyrics)} aria-pressed={showLyrics} aria-label={t('enceintes.paroles.titre')} title={t('enceintes.paroles.titre')}><Icon name="lyrics" size={24} /></button>
      {/if}
      <button class="soft" class:active={volumes} onclick={() => (volumes = !volumes)} aria-pressed={volumes} aria-label={t('enceintes.immersion.volumes')} title={t('enceintes.immersion.volumes')}><Icon name="volume-high" size={24} /></button>
    </div>
  </footer>

  {#if volumes}
    <aside class="mixer" aria-label={t('enceintes.immersion.volumes')}>
      {#each members as m (m.id)}
        {@const volume = dragging[m.id] ?? value(m.id, 'volume')}
        <div class="room" class:down={!reachable(m.id)}>
          <b>{nameOf(m.id)}</b>
          <div class="vol">
            <button class="soft small" disabled={!reachable(m.id) || volume == null} onclick={() => stepVolume(m.id, -STEP)} aria-label={t('enceintes.baisser', { room: nameOf(m.id) })}><Icon name="volume-minus" size={18} /></button>
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
            <button class="soft small" disabled={!reachable(m.id) || volume == null} onclick={() => stepVolume(m.id, STEP)} aria-label={t('enceintes.monter', { room: nameOf(m.id) })}><Icon name="volume-plus" size={18} /></button>
            <span class="num">{num(volume)}</span>
          </div>
        </div>
      {/each}
    </aside>
  {/if}
</div>

<style>
  @property --p {
    syntax: '<percentage>';
    inherits: true;
    initial-value: 0%;
  }

  .immersion {
    --ink: #fff;
    --ink-2: rgb(255 255 255 / 80%);
    --ink-3: rgb(255 255 255 / 46%);
    position: fixed;
    inset: 0;
    z-index: 200;
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) auto;
    padding: max(16px, env(safe-area-inset-top)) clamp(16px, 4vw, 56px) max(18px, env(safe-area-inset-bottom));
    color: #fff;
    background: #0b0d12;
    isolation: isolate;
    overflow: hidden;
    animation: enter 0.45s var(--ease);
  }

  .immersion.idle {
    cursor: none;
  }

  @keyframes enter {
    from {
      opacity: 0;
      transform: scale(1.03);
    }
  }

  .backdrop {
    position: absolute;
    inset: -25%;
    z-index: -2;
    background-size: cover;
    background-position: center;
    filter: blur(70px) saturate(1.9) brightness(0.6);
    animation: fade 1.2s var(--ease);
  }

  .veil {
    position: absolute;
    inset: 0;
    z-index: -1;
    background: linear-gradient(180deg, rgb(6 8 12 / 35%), rgb(6 8 12 / 72%));
  }

  @keyframes fade {
    from {
      opacity: 0;
    }
  }

  .top,
  .bottom,
  .mixer {
    transition:
      opacity 0.5s var(--ease),
      transform 0.5s var(--ease);
  }

  .idle .top {
    opacity: 0;
    transform: translateY(-12px);
    pointer-events: none;
  }

  .idle .bottom {
    opacity: 0.0;
    transform: translateY(12px);
    pointer-events: none;
  }

  .top {
    display: flex;
    align-items: center;
    gap: 14px;
  }

  .picks {
    display: flex;
    gap: 8px;
    overflow-x: auto;
    scrollbar-width: none;
  }

  .picks .chip {
    flex: none;
    border: 0;
    padding: 9px 16px;
    border-radius: 999px;
    background: rgb(255 255 255 / 12%);
    color: #fff;
    font: inherit;
    font-size: 14px;
    backdrop-filter: blur(12px);
  }

  .picks .chip.on {
    background: #fff;
    color: #10131a;
    font-weight: 650;
  }

  button.soft,
  button.round {
    border: 0;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: #fff;
    background: rgb(255 255 255 / 12%);
    backdrop-filter: blur(12px);
    width: 52px;
    height: 52px;
    cursor: pointer;
  }

  button.soft:hover:not(:disabled) {
    background: rgb(255 255 255 / 22%);
  }

  button.soft.active {
    background: #fff;
    color: #10131a;
  }

  button.soft:disabled {
    opacity: 0.35;
    cursor: default;
  }

  button.soft.small {
    width: 40px;
    height: 40px;
  }

  button.round {
    width: 76px;
    height: 76px;
    background: #fff;
    color: #10131a;
    box-shadow: 0 14px 36px -10px rgb(0 0 0 / 60%);
  }

  /* The stage: cover and title on one side, the words on the other (the cover
     alone, centred, when there are none). */
  .stage {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1.15fr);
    align-items: center;
    gap: clamp(24px, 6vw, 96px);
    min-height: 0;
  }

  .stage.solo {
    grid-template-columns: minmax(0, 1fr);
    justify-items: center;
  }

  .deck {
    display: grid;
    gap: clamp(14px, 3vh, 32px);
    justify-items: center;
    min-width: 0;
    width: 100%;
  }

  .art {
    position: relative;
    width: min(100%, 62vh, 560px);
    aspect-ratio: 1;
  }

  .art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    border-radius: 4%;
    display: block;
  }

  .art .glow {
    position: absolute;
    inset: 6%;
    width: 88%;
    height: 88%;
    filter: blur(44px) saturate(1.9);
    opacity: 0.7;
    pointer-events: none;
  }

  .art .face {
    position: relative;
    box-shadow: 0 30px 70px -20px rgb(0 0 0 / 75%);
    animation: pop 0.7s var(--ease);
    transition: transform 0.8s var(--ease);
  }

  .art .face:not(.breathe) {
    transform: scale(0.94);
  }

  @keyframes pop {
    from {
      opacity: 0;
      transform: scale(0.9);
    }
  }

  .noart {
    width: 100%;
    height: 100%;
    border-radius: 4%;
    display: grid;
    place-items: center;
    background: rgb(255 255 255 / 10%);
    color: var(--ink-3);
  }

  .what {
    width: min(100%, 560px);
    min-width: 0;
  }

  .what h2 {
    margin: 0;
    font-size: clamp(22px, 3.4vw, 42px);
    line-height: 1.15;
    font-weight: 700;
    overflow: hidden;
  }

  .what h2.dim,
  .dim {
    color: var(--ink-3);
  }

  .what .sub {
    margin: 4px 0 0;
    font-size: clamp(16px, 2vw, 24px);
    color: var(--ink-2);
    overflow: hidden;
  }

  .stage.solo .what {
    text-align: center;
  }

  .words {
    position: relative;
    height: 100%;
    max-height: 100%;
    overflow-y: auto;
    scrollbar-width: none;
    padding: 28vh 0 55vh;
    mask-image: linear-gradient(transparent, #000 18%, #000 82%, transparent);
  }

  .words p {
    margin: 0;
    padding: 0.2em 0;
    font-size: clamp(24px, 3.6vw, 52px);
    font-weight: 750;
    line-height: 1.2;
    letter-spacing: -0.01em;
    color: var(--ink-3);
    opacity: 0.55;
    filter: blur(1.5px);
    transform-origin: left center;
    transition:
      color 0.4s var(--ease),
      opacity 0.4s var(--ease),
      filter 0.4s var(--ease),
      transform 0.4s var(--ease);
  }

  .words p.near {
    opacity: 0.8;
    filter: blur(0.6px);
  }

  .words p.now {
    color: #fff;
    opacity: 1;
    filter: none;
    transform: scale(1.04);
  }

  /* The fill that follows the singer (the whole line stays white when the
     lyrics are not timed). */
  .words p.now .k {
    background: linear-gradient(90deg, #fff var(--p), rgb(255 255 255 / 42%) var(--p));
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
    transition: --p 0.12s linear;
  }

  .words p.pause .k {
    display: none;
  }

  .words p.pause {
    display: flex;
    gap: 10px;
    padding: 0.5em 0;
    opacity: 0;
    filter: none;
    transform: none;
    height: 1.4em;
    align-items: center;
  }

  .words p.pause.now {
    opacity: 1;
  }

  .words p.pause i {
    width: 0.5em;
    height: 0.5em;
    border-radius: 50%;
    background: #fff;
    animation: dots 1.8s ease-in-out infinite;
  }

  .words p.pause i:nth-child(2) {
    animation-delay: 0.3s;
  }

  .words p.pause i:nth-child(3) {
    animation-delay: 0.6s;
  }

  @keyframes dots {
    0%,
    100% {
      opacity: 0.25;
      transform: scale(0.8);
    }
    50% {
      opacity: 1;
      transform: scale(1.1);
    }
  }

  .words:not(:has(.now)) p {
    color: var(--ink-2);
    opacity: 0.9;
    filter: none;
  }

  .bottom {
    display: grid;
    gap: 14px;
    justify-items: center;
  }

  .bar {
    width: min(100%, 900px);
    display: flex;
    align-items: center;
    gap: 14px;
    font-size: 13px;
    color: var(--ink-2);
  }

  .progress {
    flex: 1;
    height: 6px;
    border-radius: 99px;
    background: rgb(255 255 255 / 22%);
    overflow: hidden;
  }

  .progress span {
    display: block;
    height: 100%;
    border-radius: 99px;
    background: #fff;
    transition: width 0.5s linear;
  }

  .transport {
    width: min(100%, 900px);
    display: flex;
    align-items: center;
    justify-content: center;
    gap: clamp(8px, 2vw, 22px);
  }

  .gap {
    flex: 1;
    max-width: 80px;
  }

  .mixer {
    position: absolute;
    right: clamp(16px, 4vw, 56px);
    bottom: 150px;
    width: min(380px, calc(100% - 32px));
    display: grid;
    gap: 14px;
    padding: 16px 18px;
    border-radius: 22px;
    background: rgb(20 22 30 / 70%);
    backdrop-filter: blur(24px);
    box-shadow: 0 24px 60px -20px rgb(0 0 0 / 70%);
  }

  .room {
    display: grid;
    gap: 6px;
  }

  .room.down {
    opacity: 0.5;
  }

  .vol {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .vol input {
    flex: 1;
    min-width: 0;
    accent-color: #fff;
  }

  .vol .num {
    min-width: 2.4ch;
    text-align: right;
  }

  /* A phone: the cover on top, the words under it. */
  @media (max-width: 860px) {
    .stage {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: auto minmax(0, 1fr);
      align-items: start;
      gap: 12px;
    }

    .stage.solo {
      align-items: center;
    }

    .art {
      width: min(100%, 34vh);
    }

    .words {
      padding: 10vh 0;
    }

    .words p {
      font-size: clamp(22px, 6vw, 34px);
    }

    button.soft,
    button.round {
      width: 46px;
      height: 46px;
    }

    button.round {
      width: 64px;
      height: 64px;
    }

    .gap {
      display: none;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .immersion,
    .backdrop,
    .art .face {
      animation: none;
    }
  }
</style>

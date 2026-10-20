<script>
  import { onMount } from 'svelte';
  import { hub, device, value, reachable, num, act } from '../lib/home.svelte.js';
  import { energy, useEnergy, watts, euros } from '../lib/energy-live.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  let { id, name } = $props();

  const d = $derived(device(id));
  const point = (key) => d?.points.find((p) => p.key === key);
  // What the unit takes, as it declares it (a Hitachi goes to 32°, a Daikin to 30°).
  const MIN = $derived(point('target_temperature')?.kind?.min ?? 16);
  const MAX = $derived(point('target_temperature')?.kind?.max ?? 30);
  const STEP = $derived(point('target_temperature')?.kind?.step ?? 0.5);
  // What was just asked shows at once, until the unit says it too.
  const asked = (key, real) => hub.pending[`${id}/${key}`] ?? real;
  const on = $derived(asked('on', value(id, 'on')) === true);
  const temp = $derived(value(id, 'temperature'));
  const target = $derived(value(id, 'target_temperature'));
  const mode = $derived(asked('mode', value(id, 'mode')));
  const fan = $derived(asked('fan_speed', value(id, 'fan_speed')));
  const outdoor = $derived(value(id, 'outdoor_temperature'));
  const fault = $derived(value(id, 'fault') === true);
  const canMode = $derived(!!point('mode')?.access?.write);
  const fans = $derived(point('fan_speed')?.access?.write ? (point('fan_speed')?.kind?.values ?? []) : []);
  /** The modes this unit takes, in the order the card shows them. */
  const modes = $derived(
    Object.entries(MODES).filter(([key]) => !point('mode')?.kind?.values || point('mode').kind.values.includes(key)),
  );
  /** A fan-only unit has no setpoint to turn. */
  const noSetpoint = $derived(mode === 'ventilation');

  // What it draws, when a measured circuit feeds it (Daikin units do not
  // measure themselves).
  onMount(useEnergy);
  const feeder = $derived(energy.summary?.meters.find((m) => m.feeds?.includes(id)));
  const draw = $derived.by(() => {
    if (!feeder?.power) return null;
    const i = feeder.power.indexOf('/');
    const v = value(feeder.power.slice(0, i), feeder.power.slice(i + 1));
    return typeof v === 'number' ? v : (feeder.power_w ?? null);
  });
  // What it cost, from its circuit's meter (today, this month).
  const costOf = (span) => (feeder ? (energy.summary?.[span]?.meters?.[feeder.id]?.cost ?? null) : null);
  const dayCost = $derived(costOf('today'));
  const monthCost = $derived(costOf('month'));
  // The circuit feeds something else too (« PAC salon + pompe piscine »).
  const shared = $derived(!!feeder && (feeder.feeds.length > 1 || feeder.name.includes('+')));
  const writable = $derived(!!point('target_temperature')?.access?.write && !noSetpoint);

  // Taps on − / + and drags on the dial add up; one order leaves once the
  // finger rests (each order re-reads the unit and sends all its settings: no
  // machine-gunning it). Off, the unit keeps the new setpoint for later.
  let wanted = $state(null);
  let dragging = $state(false);
  let timer;
  const shownTarget = $derived(wanted ?? target);
  const clamp = (deg) => Math.min(MAX, Math.max(MIN, Math.round(deg / STEP) * STEP));

  function nudge(delta) {
    const base = wanted ?? (typeof target === 'number' ? target : 21);
    wanted = clamp(base + delta);
    send(900);
  }

  function send(after) {
    clearTimeout(timer);
    timer = setTimeout(flush, after);
  }

  async function flush() {
    clearTimeout(timer);
    const v = wanted;
    if (v == null || v === target) {
      wanted = null;
      return;
    }
    const done = await act(`${id}/target_temperature`, v, t('commun.clim.label_consigne', { name }));
    // Shown until the unit reports it (or gives up on it).
    if (done !== 'ok') wanted = null;
    else setTimeout(() => wanted === v && (wanted = null), 15_000);
  }

  $effect(() => {
    if (wanted != null && !dragging && target === wanted) wanted = null;
  });

  // The dial follows the finger: the angle on the 270° ring is the setpoint.
  function tempAt(e, box) {
    const x = e.clientX - (box.left + box.width / 2);
    const y = e.clientY - (box.top + box.height / 2);
    // Too near the middle, the angle means nothing.
    if (Math.hypot(x, y) < box.width * 0.15) return null;
    let a = (((Math.atan2(y, x) * 180) / Math.PI - 135) % 360 + 360) % 360;
    // In the gap at the bottom: the nearest end.
    if (a > 270) a = a > 315 ? 0 : 270;
    return clamp(MIN + (a / 270) * (MAX - MIN));
  }

  function grab(e) {
    if (!writable || !reachable(id)) return;
    dragging = true;
    clearTimeout(timer);
    e.currentTarget.setPointerCapture(e.pointerId);
    wanted = tempAt(e, e.currentTarget.getBoundingClientRect()) ?? shownTarget;
  }

  function drag(e) {
    if (!dragging) return;
    wanted = tempAt(e, e.currentTarget.getBoundingClientRect()) ?? wanted;
  }

  function release() {
    if (!dragging) return;
    dragging = false;
    send(250);
  }

  function key(e) {
    const delta = { ArrowUp: STEP, ArrowRight: STEP, ArrowDown: -STEP, ArrowLeft: -STEP }[e.key];
    if (!delta || !writable) return;
    e.preventDefault();
    nudge(delta);
  }

  async function power() {
    // A setpoint just dragged leaves first: the unit starts on it.
    if (!on && wanted != null) await flush();
    act(`${id}/on`, !on, t(on ? 'commun.clim.label_arret' : 'commun.clim.label_marche', { name }));
  }
  const setMode = (key) => act(`${id}/mode`, key, t('commun.action', { label: name, action: modeLabel(key).toLowerCase() }));
  const setFan = (speed) => act(`${id}/fan_speed`, speed, t('commun.action', { label: name, action: t('commun.clim.vitesse_nom', { speed }) }));

  // The keys are the unit's modes (sent as they are); `label` is a catalogue key.
  const MODES = {
    froid: { label: 'commun.clim.mode.froid', short: 'commun.clim.mode_court.froid', icon: 'snowflake', tone: 'cool' },
    chaud: { label: 'commun.clim.mode.chaud', short: 'commun.clim.mode_court.chaud', icon: 'fire', tone: 'warm' },
    auto: { label: 'commun.clim.mode.auto', short: 'commun.clim.mode_court.auto', icon: 'sparkles', tone: 'good' },
    déshumidification: { label: 'commun.clim.mode.deshumidification', short: 'commun.clim.mode_court.deshumidification', icon: 'humidity', tone: 'dry' },
    ventilation: { label: 'commun.clim.mode.ventilation', short: 'commun.clim.mode_court.ventilation', icon: 'fan', tone: 'fan' },
  };
  /** A mode said in the house's language (an unknown one: as the unit says it). */
  const modeLabel = (key) => (MODES[key] ? t(MODES[key].label) : (key ?? '—'));
  const m = $derived(MODES[mode] ? { ...MODES[mode], label: modeLabel(mode) } : { label: mode ?? '—', icon: 'air-conditioner', tone: 'good' });

  // A 270° ring: the target as a bead, the room temperature in the middle.
  const R = 62;
  const C = 2 * Math.PI * R;
  const ARC = 0.75;
  const frac = (deg) => Math.min(1, Math.max(0, ((deg ?? MIN) - MIN) / (MAX - MIN)));
  const at = (deg) => {
    const a = (135 + frac(deg) * 270) * (Math.PI / 180);
    return { x: 80 + R * Math.cos(a), y: 80 + R * Math.sin(a) };
  };
  const bead = $derived(at(shownTarget));
  const here = $derived(typeof temp === 'number' ? at(temp) : null);
</script>

<div class="clim" class:on class:unreachable={!reachable(id)} data-tone={on ? m.tone : 'off'}>
  <header>
    <span class="badge"><Icon name={on ? m.icon : 'air-conditioner'} size={22} /></span>
    <div class="title">
      <h3>{name}</h3>
      <p class="status">
        {#if !reachable(id)}
          {t('commun.clim.hors_ligne')}
        {:else if on}
          {m.label}{#if fan && fans.length} · {t('commun.clim.ventilation_courte', { speed: fan === 'auto' ? t('commun.clim.vitesse_auto') : fan })}{/if}
        {:else}
          {t('commun.clim.eteinte')}
        {/if}
        {#if fault}<span class="fault"><Icon name="alert" size={13} />{t('commun.clim.defaut')}</span>{/if}
      </p>
    </div>
    {#if point('on')?.access?.write}
      <button class="power" onclick={power} aria-pressed={on} aria-label={t(on ? 'commun.clim.label_arret' : 'commun.clim.label_marche', { name })}>
        <Icon name="power" size={22} />
      </button>
    {/if}
  </header>

  <div class="body">
    <div
      class="dial"
      class:live={writable}
      class:dragging
      onpointerdown={grab}
      onpointermove={drag}
      onpointerup={release}
      onpointercancel={release}
      onkeydown={key}
      role={writable ? 'slider' : undefined}
      tabindex={writable ? 0 : undefined}
      aria-label={writable ? t('commun.clim.consigne_nom', { name }) : undefined}
      aria-valuemin={writable ? MIN : undefined}
      aria-valuemax={writable ? MAX : undefined}
      aria-valuenow={writable ? shownTarget : undefined}>
      <svg viewBox="0 0 160 160" aria-hidden="true">
        <circle class="track" cx="80" cy="80" r={R} stroke-dasharray="{C * ARC} {C}" transform="rotate(135 80 80)" />
        {#if on && shownTarget != null && !noSetpoint}
          <circle class="fill" cx="80" cy="80" r={R} stroke-dasharray="{C * ARC * frac(shownTarget)} {C}" transform="rotate(135 80 80)" />
        {/if}
        {#if here}
          <circle class="here" cx={here.x} cy={here.y} r="4.5"><title>{t('commun.clim.temp_piece')}</title></circle>
        {/if}
        {#if shownTarget != null && !noSetpoint}
          <circle class="bead" cx={bead.x} cy={bead.y} r={dragging ? 10 : 8} />
        {/if}
      </svg>
      <div class="reading">
        {#if dragging}
          <strong class="num">{num(wanted, wanted % 1 ? 1 : 0)}<small>°</small></strong>
          <span>{t('commun.clim.consigne')}</span>
        {:else}
          <strong class="num">{num(temp, temp % 1 ? 1 : 0)}<small>°</small></strong>
          <span>
            {#if shownTarget != null && !noSetpoint}{t('commun.clim.consigne_val', { temp: num(shownTarget, shownTarget % 1 ? 1 : 0) })}{:else}{t('commun.clim.temp_piece')}{/if}
          </span>
        {/if}
      </div>
    </div>

    <div class="controls">
      {#if writable}
        <div class="setpoint" role="group" aria-label={t('commun.clim.consigne')}>
          <button onclick={() => nudge(-STEP)} aria-label={t('commun.clim.moins_chaud')} disabled={shownTarget <= MIN}>−</button>
          <b class="num">{num(shownTarget, shownTarget % 1 ? 1 : 0)}°</b>
          <button onclick={() => nudge(STEP)} aria-label={t('commun.clim.plus_chaud')} disabled={shownTarget >= MAX}>+</button>
        </div>
      {:else if noSetpoint && canMode}
        <p class="hint">{t('commun.clim.pas_de_consigne')}</p>
      {/if}
      <p class="muted outside">
        {#if outdoor != null}{t('commun.clim.dehors', { temp: num(outdoor, 0) })}{/if}
        {#if draw != null && draw >= 5}
          <span class="draw num" title={t('commun.clim.mesure_tableau', { name: feeder.name })}>
            {#if outdoor != null}·{/if} {watts(draw)}{#if shared}<small> {t('commun.clim.circuit_partage_paren')}</small>{/if}
          </span>
        {/if}
      </p>
    </div>
  </div>

  {#if canMode}
    <div class="modes" style="--n:{modes.length}" role="group" aria-label={t('commun.clim.mode')}>
      {#each modes as [key, it] (key)}
        <button class="mode" data-tone={it.tone} class:active={mode === key} onclick={() => setMode(key)} aria-label={t(it.label)} aria-pressed={mode === key}>
          <Icon name={it.icon} size={20} />
          <span>{t(it.short)}</span>
        </button>
      {/each}
    </div>
  {/if}

  {#if fans.length}
    <div class="fans" role="group" aria-label={t('commun.clim.ventilation_titre')}>
      <span class="fans-label"><Icon name="fan" size={15} />{t('commun.clim.ventilation_titre')}</span>
      <div class="speeds">
        {#each fans as speed (speed)}
          <button class:active={fan === speed} onclick={() => setFan(speed)} aria-pressed={fan === speed} aria-label={speed === 'auto' ? t('commun.clim.vitesse_auto') : t('commun.clim.vitesse_nom', { speed })}>
            {#if speed === 'auto'}
              <span class="auto">A</span>
            {:else}
              <span class="bars" aria-hidden="true">
                {#each fans.filter((f) => f !== 'auto') as _, k (k)}<i class:lit={k < Number(speed)} style="height:{5 + k * 3}px"></i>{/each}
              </span>
            {/if}
          </button>
        {/each}
      </div>
    </div>
  {/if}

  {#if energy.summary}
    <p class="costs">
      {#if feeder}
        <span>{t('commun.clim.aujourdhui')} <b class="num">{euros(dayCost, energy.summary.currency)}</b></span>
        <span>{t('commun.clim.ce_mois')} <b class="num">{euros(monthCost, energy.summary.currency)}</b></span>
        {#if shared}<small>{t('commun.clim.circuit_partage')}</small>{/if}
      {:else}
        <small>{t('commun.clim.cout_non_mesure')}</small>
      {/if}
    </p>
  {/if}
</div>

<style>
  .clim {
    min-width: 0;
    --tone: var(--ink-3);
    --tone-soft: var(--surface-2);
    container-type: inline-size;
    position: relative;
    display: grid;
    gap: 14px;
    padding: 16px;
    border-radius: var(--r-lg);
    border: 1px solid var(--line);
    background: var(--surface);
    box-shadow: var(--shadow);
    overflow: hidden;
    transition: background 0.5s var(--ease), border-color 0.5s var(--ease);
  }

  .clim[data-tone='cool'] { --tone: var(--cool); }
  .clim[data-tone='warm'] { --tone: var(--warm); }
  .clim[data-tone='good'] { --tone: var(--good); }
  .clim[data-tone='dry'] { --tone: #2fa7b8; }
  .clim[data-tone='fan'] { --tone: #8b7cf6; }

  /* Lit, the whole card takes the mode's colour, softly. */
  .clim.on {
    border-color: color-mix(in srgb, var(--tone) 38%, var(--line));
    background:
      radial-gradient(120% 90% at 0% 0%, color-mix(in srgb, var(--tone) 20%, transparent), transparent 62%),
      linear-gradient(180deg, color-mix(in srgb, var(--tone) 7%, var(--surface)), var(--surface));
  }

  header {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .badge {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    flex: none;
    border-radius: 15px;
    background: var(--surface-2);
    color: var(--ink-3);
    transition: background 0.4s var(--ease), color 0.4s var(--ease);
  }

  .on .badge {
    background: color-mix(in srgb, var(--tone) 18%, var(--surface));
    color: var(--tone);
  }

  .title {
    flex: 1;
    min-width: 0;
  }

  h3 {
    font-size: 17px;
    font-weight: 700;
    letter-spacing: -0.01em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .status {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 10px;
    margin-top: 1px;
    font-size: 13px;
    font-weight: 600;
    color: var(--ink-3);
  }

  .on .status {
    color: var(--tone);
  }

  .fault {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 1px 8px;
    border-radius: 99px;
    background: var(--alert-soft);
    color: var(--alert);
    font-size: 12px;
  }

  .power {
    display: grid;
    place-items: center;
    flex: none;
    width: 46px;
    height: 46px;
    border: none;
    border-radius: 50%;
    background: var(--surface-3);
    color: var(--ink-2);
    cursor: pointer;
    transition: transform 0.2s var(--ease), background 0.3s var(--ease), box-shadow 0.3s var(--ease);
  }

  .power:hover {
    transform: scale(1.06);
  }

  .power:active {
    transform: scale(0.95);
  }

  .on .power {
    background: var(--tone);
    color: #fff;
    box-shadow: 0 6px 18px -4px color-mix(in srgb, var(--tone) 70%, transparent);
  }

  .body {
    display: grid;
    grid-template-columns: 148px 1fr;
    align-items: center;
    gap: 16px;
  }

  .dial {
    position: relative;
    width: 148px;
    height: 148px;
    justify-self: center;
  }

  svg {
    width: 100%;
    height: 100%;
  }

  .track,
  .fill {
    fill: none;
    stroke-width: 11;
    stroke-linecap: round;
  }

  .track {
    stroke: var(--surface-3);
  }

  .fill {
    stroke: var(--tone);
    transition: stroke-dasharray 0.6s var(--ease);
    filter: drop-shadow(0 0 6px color-mix(in srgb, var(--tone) 55%, transparent));
  }

  .bead {
    fill: var(--surface);
    stroke: var(--ink-3);
    stroke-width: 4;
    transition: r 0.15s var(--ease);
    filter: drop-shadow(0 1px 2px rgb(0 0 0 / 25%));
  }

  .on .bead {
    stroke: var(--tone);
  }

  /* The room's temperature: a grey dot on the ring. */
  .here {
    fill: var(--ink-3);
    stroke: var(--surface);
    stroke-width: 2;
  }

  .dial.live {
    cursor: grab;
    touch-action: none;
    user-select: none;
    -webkit-user-select: none;
    border-radius: 50%;
  }

  .dial.dragging {
    cursor: grabbing;
  }

  .dial.live:focus-visible {
    outline: 3px solid var(--cool);
    outline-offset: 2px;
  }

  .dial.dragging .reading strong {
    color: var(--tone);
  }

  .reading {
    position: absolute;
    inset: 0;
    display: grid;
    place-content: center;
    text-align: center;
    pointer-events: none;
  }

  .reading strong {
    font-size: 38px;
    font-weight: 300;
    letter-spacing: -0.03em;
    line-height: 1;
  }

  .reading small {
    font-size: 20px;
    vertical-align: top;
  }

  .reading span {
    max-width: 96px;
    margin: 5px auto 0;
    font-size: 12px;
    color: var(--ink-3);
    font-weight: 600;
    line-height: 1.25;
  }

  .controls {
    display: grid;
    gap: 10px;
    justify-items: start;
  }

  .setpoint {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px;
    border-radius: 99px;
    background: var(--surface-2);
  }

  .setpoint button {
    display: grid;
    place-items: center;
    width: 38px;
    height: 38px;
    border-radius: 50%;
    border: none;
    background: var(--surface);
    color: var(--ink);
    font: inherit;
    font-size: 22px;
    line-height: 1;
    cursor: pointer;
    box-shadow: 0 1px 2px rgb(0 0 0 / 10%);
    transition: transform 0.15s var(--ease);
  }

  .setpoint button:active:not(:disabled) {
    transform: scale(0.9);
  }

  .setpoint button:disabled {
    opacity: 0.35;
    cursor: default;
  }

  .setpoint b {
    min-width: 56px;
    text-align: center;
    font-size: 21px;
    font-weight: 650;
  }

  .hint,
  .outside {
    font-size: 12.5px;
    color: var(--ink-3);
  }

  .draw {
    color: var(--warm-ink);
    font-weight: 650;
  }

  .draw small {
    font-weight: 500;
    color: var(--ink-3);
  }

  /* The five modes, each its own colour. */
  .modes {
    display: grid;
    grid-template-columns: repeat(var(--n, 5), minmax(0, 1fr));
    gap: 6px;
  }

  .mode {
    --c: var(--ink-3);
    display: grid;
    justify-items: center;
    gap: 4px;
    padding: 10px 2px 8px;
    border: 1px solid transparent;
    border-radius: 16px;
    background: var(--surface-2);
    color: var(--ink-2);
    font: inherit;
    font-size: 11.5px;
    font-weight: 650;
    cursor: pointer;
    transition: transform 0.15s var(--ease), background 0.25s var(--ease), color 0.25s var(--ease);
  }

  .mode[data-tone='cool'] { --c: var(--cool); }
  .mode[data-tone='warm'] { --c: var(--warm); }
  .mode[data-tone='good'] { --c: var(--good); }
  .mode[data-tone='dry'] { --c: #2fa7b8; }
  .mode[data-tone='fan'] { --c: #8b7cf6; }

  .mode :global(svg) {
    color: var(--c);
  }

  .mode span {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .mode:hover {
    transform: translateY(-1px);
  }

  .mode.active {
    background: var(--c);
    color: #fff;
    box-shadow: 0 6px 16px -6px color-mix(in srgb, var(--c) 80%, transparent);
  }

  .mode.active :global(svg) {
    color: #fff;
  }

  .fans {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .fans-label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12.5px;
    font-weight: 650;
    color: var(--ink-3);
  }

  .speeds {
    display: flex;
    gap: 5px;
  }

  .speeds button {
    display: grid;
    place-items: end center;
    width: 38px;
    height: 34px;
    padding: 0 0 7px;
    border: none;
    border-radius: 11px;
    background: var(--surface-2);
    cursor: pointer;
    transition: background 0.25s var(--ease), transform 0.15s var(--ease);
  }

  .speeds button:active {
    transform: scale(0.92);
  }

  .speeds button.active {
    background: color-mix(in srgb, var(--tone) 18%, var(--surface));
    box-shadow: inset 0 0 0 1.5px var(--tone);
  }

  .bars {
    display: flex;
    align-items: flex-end;
    gap: 2px;
    height: 18px;
  }

  .bars i {
    display: block;
    width: 3px;
    border-radius: 2px;
    background: var(--surface-3);
  }

  .bars i.lit {
    background: var(--tone);
  }

  .auto {
    align-self: center;
    font-size: 13px;
    font-weight: 750;
    color: var(--ink-2);
  }

  .active .auto {
    color: var(--tone);
  }

  .costs {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 12px;
    font-size: 12.5px;
    color: var(--ink-2);
  }

  .costs b {
    font-weight: 650;
    color: var(--ink);
  }

  .costs small {
    color: var(--ink-3);
  }

  .unreachable {
    opacity: 0.55;
  }

  /* Narrow (a phone, a room card): the dial on top, the controls beneath. */
  @container (max-width: 300px) {
    .body {
      grid-template-columns: 1fr;
    }

    .controls {
      justify-items: center;
    }

    /* Icons alone: the status line says the mode's name. */
    .mode span {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
    }

    .mode {
      padding: 12px 2px;
    }

    .fans {
      flex-direction: column;
      align-items: stretch;
    }

    .speeds {
      justify-content: space-between;
    }

    .speeds button {
      flex: 1;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .clim,
    .badge,
    .power,
    .mode,
    .fill {
      transition: none;
    }
  }
</style>

<script>
  import Icon from '../ui/Icon.svelte';
  import PointPicker from './PointPicker.svelte';
  import { hub, device, nameOf, roomOf, value, note } from '../lib/home.svelte.js';
  import { api, asHuman, alerts, CANCELLED } from '../lib/auto.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';

  /** « Préviens-moi quand… » in three choices: a device, what happens to
   *  it, where to say it. What it makes is an ordinary automation, approved
   *  by a person like any other. */

  // svelte-ignore state_referenced_locally
  let id = $state(alerts.device ?? '');
  let choice = $state(null);
  /** the number some conditions need (a threshold, hours) */
  let amount = $state(null);
  let channels = $state([]);
  let message = $state('');
  let edited = $state(false);
  let busy = $state(false);
  let sentence = $state('');
  let problems = $state([]);
  let existing = $state([]);

  const d = $derived(device(id));
  const readable = (key) => d?.points.find((p) => p.key === key && p.access?.read !== false);

  // What a device can tell, the commonest first. `to`: the value that
  // matters; `below`/`above`: a threshold, with its default.
  const EVENTS = [
    ['smoke', 'fumee', 'fire', true],
    ['water_leak', 'fuite', 'water', true],
    ['watersensor_state', 'fuite', 'water', 'alarm'],
    ['gas', 'gaz', 'fire', true],
    ['carbon_monoxide', 'co', 'fire', true],
    ['contact', 'ouverture', 'door-open', false],
    ['doorcontact_state', 'ouverture', 'door-open', true],
    ['occupancy', 'mouvement', 'motion', true],
    ['motion', 'mouvement', 'motion', true],
    ['pir', 'mouvement', 'motion', 'pir'],
    ['doorbell', 'sonnette', 'bell', true],
    ['person', 'personne', 'account', true],
    ['tamper', 'arrache', 'warning', true],
    ['test', 'autotest', 'check', true],
    ['battery_low', 'pile_faible', 'battery-low', true],
  ];
  const MEASURES = [
    ['battery', 'pile_sous', 'battery-low', 'below', 15],
    ['battery_percentage', 'pile_sous', 'battery-low', 'below', 15],
    ['temperature', 'chaud', 'thermometer', 'above', 28],
    ['temperature', 'froid', 'snowflake', 'below', 5],
    ['humidity', 'humide', 'humidity', 'above', 70],
  ];

  const choices = $derived.by(() => {
    if (!d) return [];
    const out = [];
    const seen = new Set();
    for (const [key, cid, icon, to] of EVENTS) {
      if (!readable(key) || seen.has(cid)) continue;
      seen.add(cid);
      out.push({ id: cid, icon, step: { type: 'when_state', point: `${d.id}/${key}`, to, for_s: 0 } });
    }
    for (const [key, cid, icon, side, by] of MEASURES) {
      if (!readable(key) || seen.has(cid)) continue;
      seen.add(cid);
      out.push({ id: cid, icon, amount: by, unit: readable(key).unit ?? '', step: { type: 'when_threshold', point: `${d.id}/${key}`, [side]: by, for_s: 0 } });
    }
    out.push({ id: 'silencieux', icon: 'history', amount: 24, unit: 'h', step: { type: 'when_silent', device: d.id, hours: 24 } });
    out.push({ id: 'injoignable', icon: 'offline', step: { type: 'when_offline', device: d.id, for_s: 600 } });
    return out;
  });

  // Where words can go in this house.
  const has = (key) => Object.values(hub.devices).filter((x) => x.points.some((p) => p.key === key && p.access?.write));
  const messengers = $derived(has('notify'));
  const CHANNELS = $derived(
    [
      messengers.length && { id: 'telegram', icon: 'send', label: messengers.map((x) => nameOf(x.id)).join(', ') },
      { id: 'maison', icon: 'home', label: t('automatismes.alerte.canal_maison') },
      has('announce').length && { id: 'voix', icon: 'speaker', label: t('automatismes.alerte.canal_voix') },
      has('push').length && { id: 'telephone', icon: 'phone', label: t('automatismes.alerte.canal_telephone') },
    ].filter(Boolean),
  );

  // svelte-ignore state_referenced_locally
  channels = messengers.length ? ['telegram'] : ['maison'];

  function pickDevice(next) {
    id = next;
    choice = null;
  }

  function pick(c) {
    choice = c;
    amount = c.amount ?? null;
    edited = false;
  }

  const step = $derived.by(() => {
    if (!choice) return null;
    const s = { ...choice.step };
    if (amount != null) {
      if (s.type === 'when_silent') s.hours = Math.max(1, Math.round(Number(amount) || 1));
      else if ('below' in s) s.below = Number(amount);
      else if ('above' in s) s.above = Number(amount);
    }
    return s;
  });

  const what = $derived(choice ? t(`automatismes.alerte.si.${choice.id}`, { valeur: amount ?? '', unite: choice.unit ?? '' }) : '');
  const where = $derived(d ? (roomOf(d) ? `${nameOf(d.id)} (${roomOf(d)})` : nameOf(d.id)) : '');
  // The message follows the choices until someone writes their own.
  $effect(() => {
    if (!choice || edited) return;
    message = `${t(`automatismes.alerte.emoji.${choice.id}`)} ${what} : ${where}`;
  });

  const graph = $derived(
    step && {
      nodes: [
        { id: 't1', ...step, x: 60, y: 200 },
        { id: 'a1', type: 'notify', title: null, message, channels, x: 340, y: 200 },
      ],
      edges: [{ from: 't1', port: 'out', to: 'a1' }],
    },
  );
  const name = $derived(choice && d ? `${what} · ${nameOf(d.id)}` : '');

  // The server says it in words, and what is wrong.
  let timer;
  $effect(() => {
    const g = graph;
    clearTimeout(timer);
    if (!g || !message.trim()) {
      sentence = '';
      problems = [];
      return;
    }
    timer = setTimeout(async () => {
      try {
        const c = await api.check(g);
        sentence = c.summary;
        problems = (c.problems ?? []).filter((p) => p.level === 'error');
      } catch {
        sentence = '';
      }
    }, 250);
  });

  // The alerts already about this device.
  $effect(() => {
    const target = id;
    existing = [];
    if (!target) return;
    api
      .list()
      .then((r) => {
        if (id !== target) return;
        existing = r.automations.filter((i) => JSON.stringify(i.automation.graph).includes(`"${target}`));
      })
      .catch(() => {});
  });

  function toggleChannel(c) {
    const next = channels.includes(c) ? channels.filter((x) => x !== c) : [...channels, c];
    if (next.length) channels = next;
  }

  async function create() {
    if (!graph || busy || problems.length) return;
    busy = true;
    try {
      const v = await api.create({ id: '', name, mode: 'single', enabled: false, graph, note: null });
      await asHuman(t('automatismes.alerte.valider_label', { nom: name }), () => api.approve(v.automation.id, v.fingerprint));
      note(t('automatismes.alerte.creee', { nom: name }));
      close();
    } catch (err) {
      if (err.message !== CANCELLED) note(err.message, 'error');
      // Saved but not approved: it waits in the list, as a draft.
      else note(t('automatismes.alerte.brouillon'));
      if (err.message === CANCELLED) close();
    } finally {
      busy = false;
    }
  }

  function close() {
    alerts.open = false;
    alerts.device = null;
  }

  function onkey(e) {
    if (e.key === 'Escape') close();
  }
</script>

<svelte:window onkeydown={onkey} />

<button class="scrim" onclick={close} aria-label={t('commun.fermer')}></button>
<div class="wizard" role="dialog" aria-modal="true" aria-label={t('automatismes.alerte.titre')}>
  <header>
    <span class="tile"><Icon name="bell" size={22} /></span>
    <h2>{t('automatismes.alerte.titre')}</h2>
    <button class="x" onclick={close} aria-label={t('commun.fermer')}><Icon name="close" size={20} /></button>
  </header>

  <div class="body">
    <section>
      <h3><i>1</i>{t('automatismes.alerte.equipement')}</h3>
      <PointPicker point={id} mode="device" onpick={pickDevice} />
      {#if existing.length}
        <p class="already">
          <Icon name="bell" size={14} />
          {t('automatismes.alerte.deja', { count: existing.length })}
          {#each existing as i (i.automation.id)}
            <a href="#/automatismes/{i.automation.id}" onclick={close} class:live={i.live}>{i.automation.name}</a>
          {/each}
        </p>
      {/if}
    </section>

    {#if d}
      <section>
        <h3><i>2</i>{t('automatismes.alerte.quand')}</h3>
        <div class="choices" role="radiogroup" aria-label={t('automatismes.alerte.quand')}>
          {#each choices as c (c.id)}
            <button role="radio" aria-checked={choice?.id === c.id} class:on={choice?.id === c.id} onclick={() => pick(c)}>
              <Icon name={c.icon} size={18} />
              <span>{t(`automatismes.alerte.choix.${c.id}`)}</span>
              {#if c.step.point && value(d.id, c.step.point.split('/')[1]) != null && c.step.type === 'when_threshold'}
                <small>{t('automatismes.alerte.maintenant', { valeur: value(d.id, c.step.point.split('/')[1]), unite: c.unit })}</small>
              {/if}
            </button>
          {/each}
        </div>
        {#if choice && choice.amount != null}
          <label class="amount">
            <span>{t(`automatismes.alerte.reglage.${choice.step.type === 'when_silent' ? 'heures' : 'seuil'}`)}</span>
            <input type="number" bind:value={amount} min={choice.step.type === 'when_silent' ? 1 : undefined} max={choice.step.type === 'when_silent' ? 744 : undefined} />
            <small>{choice.unit}</small>
          </label>
        {/if}
      </section>
    {/if}

    {#if choice}
      <section>
        <h3><i>3</i>{t('automatismes.alerte.prevenir')}</h3>
        <div class="channels" role="group" aria-label={t('automatismes.alerte.prevenir')}>
          {#each CHANNELS as c (c.id)}
            <button aria-pressed={channels.includes(c.id)} class:on={channels.includes(c.id)} onclick={() => toggleChannel(c.id)}>
              <Icon name={channels.includes(c.id) ? 'check' : c.icon} size={16} />{c.label}
            </button>
          {/each}
        </div>
        <label class="message">
          <span>{t('automatismes.alerte.message')}</span>
          <textarea rows="2" bind:value={message} oninput={() => (edited = true)}></textarea>
        </label>
      </section>
    {/if}
  </div>

  {#if choice}
    <footer>
      {#each problems as p, i (i)}<p class="problem"><Icon name="warning" size={14} />{p.message}</p>{/each}
      {#if sentence}<p class="sentence">{sentence}</p>{/if}
      <div class="actions">
        <button class="go" onclick={create} disabled={busy || !message.trim() || problems.length > 0}>
          <Icon name="check" size={18} />{t('automatismes.alerte.creer')}
        </button>
      </div>
      <small class="muted">{t('automatismes.alerte.code')}</small>
    </footer>
  {/if}
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 44;
    border: 0;
    background: rgb(10 14 22 / 30%);
  }

  .wizard {
    position: fixed;
    z-index: 45;
    left: 50%;
    /* Anchored at the top: it grows downwards, nothing jumps under the finger. */
    top: max(16px, 6dvh);
    transform: translateX(-50%);
    width: min(560px, calc(100vw - 32px));
    max-height: calc(100dvh - max(32px, 12dvh));
    display: grid;
    grid-template-rows: auto 1fr auto;
    gap: 14px;
    padding: 20px;
    border-radius: var(--r-xl);
    background: var(--surface);
    box-shadow: var(--shadow-lift);
  }

  header {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  header h2 {
    flex: 1;
    font-size: 20px;
    font-weight: 700;
  }

  .tile {
    width: 40px;
    height: 40px;
    border-radius: 12px;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, #b05fd8 14%, var(--surface));
    color: #b05fd8;
  }

  .x {
    width: 38px;
    height: 38px;
    border-radius: 50%;
    border: 0;
    background: var(--surface-2);
    color: var(--ink);
    display: grid;
    place-items: center;
    cursor: pointer;
  }

  .body {
    overflow-y: auto;
    display: grid;
    align-content: start;
    gap: 20px;
    padding-right: 4px;
  }

  section {
    display: grid;
    gap: 10px;
  }

  h3 {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    font-weight: 750;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ink-2);
  }

  h3 i {
    font-style: normal;
    width: 20px;
    height: 20px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: var(--warm-soft);
    color: var(--warm-ink);
    font-size: 11px;
    letter-spacing: 0;
  }

  .already {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--ink-3);
  }

  .already a {
    padding: 3px 9px;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--ink-2);
    text-decoration: none;
    font-weight: 650;
  }

  .already a.live {
    background: var(--good-soft);
    color: var(--good);
  }

  .choices {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 8px;
  }

  .choices button,
  .channels button {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: 14px;
    background: var(--surface);
    color: var(--ink);
    font: inherit;
    font-size: 14px;
    font-weight: 650;
    text-align: left;
    cursor: pointer;
  }

  .choices button {
    flex-wrap: wrap;
  }

  .choices button span {
    flex: 1;
  }

  .choices small {
    width: 100%;
    padding-left: 26px;
    font-size: 12px;
    font-weight: 600;
    color: var(--ink-3);
  }

  .choices button:hover,
  .channels button:hover {
    background: var(--surface-2);
  }

  .choices button.on,
  .channels button.on {
    border-color: var(--warm);
    background: var(--warm-soft);
    color: var(--warm-ink);
  }

  .amount {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 14px;
    color: var(--ink-2);
  }

  .amount input {
    width: 90px;
    border: 1px solid var(--line);
    border-radius: 999px;
    padding: 7px 12px;
    font: inherit;
    background: var(--surface);
    color: var(--ink);
  }

  .channels {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .channels button {
    border-radius: 999px;
  }

  .message {
    display: grid;
    gap: 6px;
    font-size: 13px;
    font-weight: 650;
    color: var(--ink-3);
  }

  textarea {
    border: 1px solid var(--line);
    border-radius: 14px;
    padding: 10px 12px;
    font: inherit;
    font-size: 14px;
    background: var(--surface);
    color: var(--ink);
    resize: vertical;
  }

  footer {
    display: grid;
    gap: 8px;
    padding-top: 12px;
    border-top: 1px solid var(--line);
  }

  .sentence {
    font-size: 14px;
    color: var(--ink-2);
    font-style: italic;
  }

  .problem {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--alert);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
  }

  .go {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    border: 0;
    border-radius: 999px;
    padding: 11px 20px;
    font: inherit;
    font-weight: 750;
    background: var(--warm);
    color: #241503;
    cursor: pointer;
  }

  .go:disabled {
    opacity: 0.5;
    cursor: default;
  }

  footer small {
    text-align: right;
    font-size: 12px;
  }
</style>

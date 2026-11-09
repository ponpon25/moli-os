<script>
  /** Anything that only switches, on one line. A plug that measures its
   *  power says it, opens its curve when touched, and asks before cutting
   *  what is drawing. */
  import { isOn, nameOf, num, reachable, setPower, value } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';
  import Spark from './infra/Spark.svelte';

  let { d, icon = 'power', name } = $props();
  const on = $derived(isOn(d.id));

  /** From this draw, switching off asks first (a computer, a freezer…). */
  const ASK_FROM_W = 20;
  const metered = $derived(d.points?.some((p) => p.key === 'power' && p.unit === 'W' && p.access?.read) ?? false);
  const watts = $derived.by(() => {
    const w = metered ? value(d.id, 'power') : null;
    return typeof w === 'number' ? Math.max(0, w) : null;
  });
  const wattsText = $derived(watts == null ? null : `${num(watts, watts < 10 ? 1 : 0)} W`);

  let open = $state(false);
  let hours = $state(24);
  let asking = $state(false);

  function toggle() {
    if (on && watts != null && watts >= ASK_FROM_W) {
      asking = true;
      return;
    }
    setPower(d, !on, nameOf(d.id));
  }
  function cut() {
    asking = false;
    setPower(d, false, nameOf(d.id));
  }

  const state = $derived(!reachable(d.id) ? t('commun.lampe.injoignable') : on ? t('maison.accueil.allume') : t('maison.accueil.eteint'));
</script>

<div class="h-line">
  <div class="h-row">
    <span class="h-ico" class:lit={on}><Icon name={icon} size={19} /></span>
    {#if metered}
      <button type="button" class="name" aria-expanded={open} title={t('maison.prise.voir_courbe', { name: nameOf(d.id) })} onclick={() => (open = !open)}>
        <b>{name}</b><Icon name={open ? 'chevron-up' : 'chevron-down'} size={14} />
      </button>
    {:else}
      <b>{name}</b>
    {/if}
    <small>{state}{#if on && wattsText}{' · '}<span class="w">{wattsText}</span>{/if}</small>
    <button class="h-switch" class:on role="switch" aria-checked={on} aria-label={nameOf(d.id)} disabled={!reachable(d.id)} onclick={toggle}><i></i></button>
  </div>
  {#if asking}
    <div class="ask" role="alertdialog" aria-label={t('maison.prise.confirmer', { name: nameOf(d.id), watts: wattsText })}>
      <span>{t('maison.prise.confirmer', { name: nameOf(d.id), watts: wattsText })}</span>
      <button type="button" class="cut" onclick={cut}>{t('maison.prise.eteindre')}</button>
      <button type="button" onclick={() => (asking = false)}>{t('maison.prise.annuler')}</button>
    </div>
  {/if}
  {#if open}
    <div class="curve">
      <div class="spans" role="radiogroup" aria-label={t('maison.prise.periode')}>
        {#each [24, 168] as h (h)}
          <button type="button" role="radio" aria-checked={hours === h} class:on={hours === h} onclick={() => (hours = h)}>{h === 24 ? t('maison.prise.h24') : t('maison.prise.j7')}</button>
        {/each}
      </div>
      {#key hours}
        <Spark point={`${d.id}/power`} {hours} label={t('maison.prise.puissance')} span={hours === 24 ? t('maison.prise.h24') : t('maison.prise.j7')} unit=" W" />
      {/key}
    </div>
  {/if}
</div>

<style>
  .name {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }

  .name :global(svg) {
    flex: none;
    color: var(--ink-3);
  }

  .w {
    font-weight: 700;
    color: var(--ink-2);
  }

  .ask {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 10px;
    margin: 6px 0 2px 30px;
    font-size: 13px;
    color: var(--ink-2);
  }

  .ask span {
    flex: 1 1 180px;
  }

  .ask button,
  .spans button {
    min-height: 30px;
    padding: 4px 12px;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--ink);
    font: inherit;
    font-size: 13px;
    font-weight: 650;
    cursor: pointer;
  }

  .ask .cut {
    border-color: transparent;
    background: var(--alert);
    color: #fff;
  }

  .curve {
    display: grid;
    gap: 6px;
    margin: 6px 0 2px 30px;
  }

  .spans {
    display: flex;
    gap: 6px;
  }

  .spans button.on {
    border-color: transparent;
    background: var(--warm);
    color: var(--warm-ink, #fff);
  }
</style>

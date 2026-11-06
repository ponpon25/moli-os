<script>
  /** Anything that only switches, on one line. */
  import { isOn, nameOf, reachable, setPower } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  let { d, icon = 'power', name } = $props();
  const on = $derived(isOn(d.id));
</script>

<div class="h-line">
  <div class="h-row">
    <span class="h-ico" class:lit={on}><Icon name={icon} size={19} /></span>
    <b>{name}</b>
    <small>{!reachable(d.id) ? t('commun.lampe.injoignable') : on ? t('maison.accueil.allume') : t('maison.accueil.eteint')}</small>
    <button class="h-switch" class:on role="switch" aria-checked={on} aria-label={nameOf(d.id)} disabled={!reachable(d.id)} onclick={() => setPower(d, !on, nameOf(d.id))}><i></i></button>
  </div>
</div>

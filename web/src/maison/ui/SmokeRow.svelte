<script>
  /** A smoke detector on one line. */
  import { value } from '../lib/home.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  let { d, name } = $props();
  const bad = $derived(value(d.id, 'smoke') === true);
</script>

<div class="h-line">
  <div class="h-row">
    <span class="h-ico" class:bad><Icon name="smoke-detector" size={19} /></span>
    <b>{name}</b>
    <small class:bad>{bad ? t('maison.accueil.fumee_alerte') : t('maison.accueil.rien_a_signaler')}</small>
  </div>
</div>

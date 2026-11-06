<script>
  /** « Personnaliser » : the same words and the same gesture on every page.
   *  `lists`: what « back to default » puts back. */
  import { resetLists } from '../lib/order.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  let { editing = $bindable(false), lists = [] } = $props();
</script>

<div class="perso">
  {#if editing}
    <button class="ghost" onclick={() => resetLists(lists)}>{t('maison.dashboard.reinitialiser')}</button>
  {/if}
  <button class:on={editing} aria-pressed={editing} onclick={() => (editing = !editing)}>
    <Icon name={editing ? 'check' : 'pencil'} size={15} />{editing ? t('maison.dashboard.terminer') : t('maison.dashboard.personnaliser')}
  </button>
</div>

<style>
  .perso {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 8px 14px;
    border: 0;
    border-radius: 999px;
    background: rgb(255 255 255 / 15%);
    color: #fff;
    font-size: 13.5px;
    font-weight: 600;
  }

  button:hover {
    background: rgb(255 255 255 / 24%);
  }

  button.on {
    background: var(--warm);
    color: #2d2108;
  }

  .ghost {
    background: none;
    color: rgb(255 255 255 / 84%);
  }
</style>

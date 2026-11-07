<script>
  /** CANAL+ live, one tap: the Apple TV wakes, opens CANAL+ and goes to the
   *  channel by itself (some 20 s). The first channels as buttons, the
   *  others in a list. */
  import { value } from '../lib/home.svelte.js';
  import { canalChannels, CANAL_FIRST, watch } from '../lib/appletv.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  let { id, compact = false } = $props();
  const all = $derived(canalChannels(id));
  const first = $derived(all.slice(0, CANAL_FIRST));
  const more = $derived(all.slice(CANAL_FIRST));
  // The one asked for, while the Apple TV gets there.
  let going = $state(null);
  let timer = null;

  async function go(channel) {
    if (!channel) return;
    going = channel;
    clearTimeout(timer);
    timer = setTimeout(() => (going = null), 22000);
    await watch(id, channel);
  }
</script>

{#if all.length}
  <section class="card canal" class:compact>
    <div class="card-head">
      <h2><Icon name="tv" size={18} />{t('salon.atv.chaines_canal')}</h2>
      {#if going}<span class="going" role="status">{t('salon.atv.en_route', { chaine: going })}</span>{/if}
    </div>
    <div class="channels" role="group" aria-label={t('salon.atv.chaines_canal')}>
      {#each first as c (c)}
        <button class:on={going === c} onclick={() => go(c)} disabled={value(id, 'state') == null}>{c}</button>
      {/each}
    </div>
    {#if more.length}
      <label class="more">
        <span>{t('salon.atv.autres_chaines')}</span>
        <select onchange={(e) => { go(e.currentTarget.value); e.currentTarget.value = ''; }}>
          <option value="">{t('salon.atv.choisir')}</option>
          {#each more as c (c)}<option value={c}>{c}</option>{/each}
        </select>
      </label>
    {/if}
  </section>
{/if}

<style>
  .canal {
    padding: 22px;
  }

  .card-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
  }

  .going {
    font-size: 13px;
    color: var(--cool);
    font-weight: 650;
  }

  .channels {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(104px, 1fr));
    gap: 8px;
    margin-top: 14px;
  }

  .compact .channels {
    grid-template-columns: repeat(auto-fill, minmax(88px, 1fr));
  }

  .channels button {
    border: 0;
    border-radius: var(--r-sm, 12px);
    padding: 12px 8px;
    font: inherit;
    font-size: 14px;
    font-weight: 700;
    background: var(--surface-2);
    color: var(--ink);
    cursor: pointer;
  }

  .channels button.on {
    background: var(--cool-soft);
    color: var(--cool);
  }

  .channels button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .more {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-top: 12px;
    font-size: 14px;
    color: var(--ink-2);
  }

  .more select {
    font: inherit;
    border-radius: 999px;
    border: 0;
    padding: 8px 14px;
    background: var(--surface-2);
    color: var(--ink);
    max-width: 60%;
  }
</style>

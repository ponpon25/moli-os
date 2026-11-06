<script>
  /** The themes of the dashboard: one click, and what each arranged stays. */
  import { look, setLook } from '../lib/look.svelte.js';
  import { THEMES, themeOf } from '../themes/index.js';
  import { t } from '../../lib/i18n.svelte.js';

  const current = $derived(themeOf(look.id).id);
</script>

<section class="card themes">
  <h2>{t('maison.theme.titre')}</h2>
  <p class="muted">{t('maison.theme.aide')}</p>
  <div class="choices" role="radiogroup" aria-label={t('maison.theme.titre')}>
    {#each THEMES as th (th.id)}
      <button role="radio" aria-checked={current === th.id} class:on={current === th.id} onclick={() => setLook(th.id)}>
        <span class="swatch" aria-hidden="true" style:background="linear-gradient(120deg, {th.preview[0]} 50%, {th.preview[1]} 50%)"></span>
        <b>{t(th.label)}</b>
        <small>{t(th.desc)}</small>
      </button>
    {/each}
  </div>
</section>

<style>
  h2 {
    margin-bottom: 4px;
    font-size: 17px;
    font-weight: 660;
  }

  .choices {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(220px, 100%), 1fr));
    gap: 10px;
    margin-top: 14px;
  }

  button {
    display: grid;
    gap: 3px;
    justify-items: start;
    padding: 12px;
    border: 0;
    border-radius: var(--r-sm);
    background: var(--surface-2);
    color: var(--ink);
    text-align: left;
  }

  button.on {
    outline: 2px solid var(--warm);
    outline-offset: -2px;
  }

  .swatch {
    width: 100%;
    height: 52px;
    margin-bottom: 6px;
    border-radius: 9px;
  }

  small {
    color: var(--ink-3);
    font-size: 13px;
  }
</style>

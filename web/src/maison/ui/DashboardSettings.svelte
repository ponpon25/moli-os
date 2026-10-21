<script>
  import { catalogue, showTile } from '../lib/tiles.svelte.js';
  import { resetDashboard } from '../lib/layout.svelte.js';
  import { t } from '../../lib/i18n.svelte.js';
  import Icon from './Icon.svelte';

  /** What the dashboard shows: one switch per tile (each clim has its own).
   *  The order and the size are set on the dashboard itself. */
  const tiles = $derived(catalogue());
  const shown = $derived(tiles.filter((x) => x.on).length);
</script>

<section class="card">
  <h2>{t('systeme.dashboard.titre')} <span class="muted">{shown}/{tiles.length}</span></h2>
  <p class="muted">{t('systeme.dashboard.aide')}</p>
  <ul>
    {#each tiles as tile (tile.id)}
      <li>
        <label>
          <input type="checkbox" checked={tile.on} onchange={(e) => showTile(tile, e.currentTarget.checked)} />
          <span>{tile.label}</span>
        </label>
      </li>
    {/each}
  </ul>
  <div class="actions">
    <a class="link" href="#/"><Icon name="pencil" size={16} />{t('systeme.dashboard.arranger')}</a>
    <button onclick={() => resetDashboard()}>{t('maison.dashboard.reinitialiser')}</button>
  </div>
</section>

<style>
  ul {
    list-style: none;
    margin: 14px 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(100%, 240px), 1fr));
    gap: 6px 18px;
  }

  label {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 4px;
    cursor: pointer;
    min-height: 40px;
  }

  input {
    width: 18px;
    height: 18px;
    accent-color: var(--accent, #f5b700);
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    align-items: center;
  }

  .link {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--ink-2);
  }

  button {
    border: 0;
    background: transparent;
    color: var(--ink-3);
    font: inherit;
    cursor: pointer;
    padding: 8px 4px;
  }
</style>

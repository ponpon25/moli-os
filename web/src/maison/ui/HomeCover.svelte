<script>
  /** A speaker's cover, through Moli (a browser away from home cannot reach a
   *  speaker), else its own address, else the speaker's icon. */
  import Icon from './Icon.svelte';

  let { id, art = null, size = 46 } = $props();
  let failed = $state({});
  const src = $derived(art && (failed[art] ?? 0) < 2 ? (failed[art] === 1 ? art : `/api/devices/${id}/cover?v=${encodeURIComponent(art)}`) : null);
</script>

{#if src}
  <img {src} alt="" style:width="{size}px" style:height="{size}px" onerror={() => (failed[art] = (failed[art] ?? 0) + 1)} />
{:else}
  <span class="none" style:width="{size}px" style:height="{size}px"><Icon name="speaker" size={Math.round(size * 0.42)} /></span>
{/if}

<style>
  img,
  .none {
    flex: none;
    display: grid;
    place-items: center;
    border-radius: 11px;
    object-fit: cover;
  }

  .none {
    background: var(--surface-3);
    color: var(--ink-3);
  }
</style>

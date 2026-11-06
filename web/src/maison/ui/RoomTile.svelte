<script>
  /** A room at a glance: its temperature, and one icon an equipment, lit when
   *  it is active. */
  import { num } from '../lib/home.svelte.js';
  import Icon from './Icon.svelte';

  let { r, active = false, onclick } = $props();
</script>

<button class="room" class:lit={r.lit} class:active aria-current={active ? 'page' : undefined} {onclick}>
  <span class="top">
    <b>{r.name}</b>
    {#if r.temp != null}<small class="num">{num(r.temp, 1)}°</small>{/if}
  </span>
  <span class="gear">
    {#each r.gear as g (g.id)}
      <span class="g" class:on={g.on} class:bad={g.bad} title={g.label}><Icon name={g.icon} size={16} /></span>
    {/each}
  </span>
</button>

<style>
  .room {
    width: 100%;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    gap: 12px;
    min-height: 92px;
    padding: 13px;
    border: 0;
    border-radius: 18px;
    background: rgb(255 255 255 / 13%);
    backdrop-filter: blur(16px);
    color: #fff;
    font: inherit;
    text-align: left;
    cursor: pointer;
    transition: background 0.2s, transform 0.2s var(--ease);
  }

  .room:hover {
    background: rgb(255 255 255 / 22%);
    transform: translateY(-2px);
  }

  .room.lit {
    background: rgb(255 190 92 / 30%);
    box-shadow: 0 8px 34px -10px #ffb24d;
  }

  .room.active {
    background: rgb(255 255 255 / 28%);
    box-shadow: inset 0 0 0 1px rgb(255 255 255 / 40%);
  }

  .top {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 6px;
  }

  b {
    font-size: 14.5px;
    font-weight: 630;
    line-height: 1.2;
  }

  small {
    color: rgb(255 255 255 / 74%);
    font-size: 13px;
  }

  .gear {
    display: flex;
    flex-wrap: wrap;
    gap: 5px 6px;
  }

  .g {
    display: block;
    color: rgb(255 255 255 / 58%);
  }

  .g.on {
    color: #ffd98e;
  }

  .g.bad {
    color: #ffb1a0;
  }
</style>

<script>
  // The dashboard's face: a theme is a frame and its pages, nothing of the
  // house. Only the one in use is loaded. See `themes/index.js`.
  import '@fontsource-variable/figtree';
  import './maison.css';
  import { look } from './lib/look.svelte.js';
  import { themeOf } from './themes/index.js';

  let { route } = $props();
</script>

{#await themeOf(look.id).load() then { default: Frame }}
  <Frame {route} />
{/await}

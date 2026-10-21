// The dashboard's tiles, one per thing the family may want to see or not:
// the house's cards, and each clim on its own. The dashboard draws them and
// the settings list them, so both read this catalogue.

import { home, hub, deviceKind, nameOf } from './home.svelte.js';
import { dash, tileOf, setTile, onDashboard, showOnDashboard } from './layout.svelte.js';
import { t } from '../../lib/i18n.svelte.js';

/** A clim's tile is `clim:` and the device's id. */
export const CLIM = 'clim:';
export const isClim = (id) => id.startsWith(CLIM);

/** The tiles that are not devices, in their default order, and their default
 *  width in columns (of 12). `clims` stands where the clims come. */
const DEFS = [
  { id: 'sky', w: 12 },
  { id: 'modes', w: 12 },
  { id: 'notices', w: 12 },
  { id: 'alarm', w: 12 },
  { id: 'lights', w: 8 },
  { id: 'energy', w: 4 },
  { id: 'routines', w: 4 },
  { id: 'clims' },
  { id: 'doorbell', w: 5 },
  { id: 'remote', w: 3 },
  { id: 'outside', w: 4 },
];

/** Two clims side by side. */
const CLIM_WIDTH = 6;

/** Every clim Moli knows: those the layout file names first, then the others
 *  (unless the house hides them), by name. */
function climDevices() {
  const cfg = home.config ?? {};
  const listed = (cfg.favorites?.climate ?? []).map((c) => ({ id: c.id, name: c.name }));
  const ids = new Set(listed.map((c) => c.id));
  const others = Object.values(hub.devices)
    .filter((d) => deviceKind(d) === 'climate' && !ids.has(d.id) && !(cfg.hidden ?? []).includes(d.id))
    .map((d) => ({ id: d.id, name: nameOf(d.id) }))
    .sort((a, b) => a.name.localeCompare(b.name));
  return [...listed, ...others];
}

/** All the tiles, shown or not, in the order they are drawn. A tile has its
 *  `id`, `label`, `on`, `w`, `h` (0: its own) and `order`; a clim's also has
 *  its `device`. Whether the house has what a tile is about is the page's to
 *  say (`isClim` tiles always have). */
export function catalogue() {
  const tiles = [];
  DEFS.forEach((d, i) => {
    if (d.id !== 'clims') {
      tiles.push({ id: d.id, label: t(`maison.dashboard.t_${d.id}`), ...tileOf(d.id, { on: true, w: d.w, h: 0, order: i }) });
      return;
    }
    // Before clims had a tile each, one tile held them all: its place, and
    // its choice to be hidden, carry over.
    const old = dash.tiles.climate ?? {};
    climDevices().forEach((c, k) => {
      const id = CLIM + c.id;
      const mine = tileOf(id, { w: CLIM_WIDTH, h: 0, order: (old.order ?? i) + k / 100 });
      tiles.push({ id, device: c.id, label: c.name, ...mine, on: onDashboard(c.id, old.on !== false) });
    });
  });
  return tiles.sort((a, b) => a.order - b.order);
}

/** Shows or hides a tile (a clim's choice is the one Équipements makes too). */
export function showTile(tile, yes) {
  if (tile.device) showOnDashboard(tile.device, yes);
  else setTile(tile.id, { on: yes });
}

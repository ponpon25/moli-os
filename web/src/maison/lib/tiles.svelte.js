// The dashboard's tiles, one per thing the family may want to see or not:
// the house's cards, and each clim on its own. The dashboard draws them and
// the settings list them, so both read this catalogue.

import { home, hub, deviceKind, nameOf, device } from './home.svelte.js';
import { dash, tileOf, setTile, onDashboard, showOnDashboard } from './layout.svelte.js';
import { t } from '../../lib/i18n.svelte.js';
import { energy } from './energy-live.svelte.js';

/** A clim's tile is `clim:` and the device's id. */
export const CLIM = 'clim:';
export const isClim = (id) => id.startsWith(CLIM);

/** Any other device the family put on the dashboard has `dev:` and its id. */
export const DEV = 'dev:';
export const isDev = (id) => id.startsWith(DEV);

/** The house's alarm: a device that takes a `mode` among which « disarmed »
 *  (its card is the dashboard's `alarm` tile). */
export const isAlarm = (d) => !!d?.points?.some((p) => p.key === 'mode' && p.kind?.values?.includes('disarmed'));

/** Whether the house has what a card is about (the others are not offered: no
 *  doorbell, no remote, no weather to show). A device's tile always has. */
export function available(id) {
  const cfg = home.config ?? {};
  const devices = Object.values(hub.devices);
  const out = cfg.outdoor ?? {};
  switch (id) {
    case 'modes':
      return !!devices.find((d) => d.id.endsWith(':mode_maison'))?.points.find((p) => p.key === 'state')?.kind.values?.length;
    case 'alarm':
      return devices.some(isAlarm);
    case 'lights':
      return true;
    case 'energy':
      return !energy.missing;
    case 'doorbell':
      return !!(cfg.favorites?.cameras ?? [])[0];
    case 'remote':
      return !!cfg.salon?.tv;
    case 'outside':
      return !!(out.air || out.weather || out.pool);
    default:
      return true;
  }
}

/** Lights and clims are on the dashboard unless taken off; everything else is
 *  off until the family puts it there. */
const byDefault = (d) => ['light', 'group', 'climate'].includes(deviceKind(d));

/** Whether a device is on the dashboard (the box in Équipements). */
export function deviceOnDashboard(d) {
  if (isAlarm(d)) return tileOf('alarm', { on: true }).on;
  return onDashboard(d.id, byDefault(d));
}

export function showDevice(d, yes) {
  if (isAlarm(d)) setTile('alarm', { on: yes });
  else showOnDashboard(d.id, yes);
}

/** The tiles that are not devices, in their default order, and their default
 *  width in columns (of 12). `clims` stands where the clims come. */
const DEFS = [
  { id: 'sky', w: 12 },
  { id: 'modes', w: 12 },
  { id: 'notices', w: 12 },
  { id: 'alarm', w: 12 },
  { id: 'lights', w: 8 },
  { id: 'ambiances', w: 8 },
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
  // Every other device the family chose to show, after the rest.
  Object.values(hub.devices)
    .filter((d) => !byDefault(d) && !isAlarm(d) && onDashboard(d.id, false))
    .sort((a, b) => nameOf(a.id).localeCompare(nameOf(b.id)))
    .forEach((d, k) => {
      const id = DEV + d.id;
      tiles.push({ id, device: d.id, label: nameOf(d.id), ...tileOf(id, { w: 4, rows: 0, order: 100 + k }), on: true });
    });
  return tiles.sort((a, b) => a.order - b.order);
}

/** Shows or hides a tile (a clim's choice is the one Équipements makes too). */
export function showTile(tile, yes) {
  if (tile.device) showOnDashboard(tile.device, yes);
  else setTile(tile.id, { on: yes });
}

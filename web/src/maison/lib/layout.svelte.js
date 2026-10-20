// What this screen remembers of the way the family arranged it: which
// rooms show, the rooms made here, the dashboard's tiles. Kept on this
// device (localStorage) and by the house (`prefs.json`), so every device sees
// the same; a device's room itself is the hub's (its label).

import { home, note, onPrefs, roomList, savePref } from './home.svelte.js';
import { t } from '../../lib/i18n.svelte.js';

function read(key, fallback) {
  try {
    const raw = localStorage.getItem(key);
    return raw ? JSON.parse(raw) : fallback;
  } catch {
    return fallback;
  }
}

const keep = (key, value) => savePref(key, JSON.stringify(value));

export const layout = $state({
  /** rooms the family hides from the Rooms page */
  hiddenRooms: read('maison-rooms-hidden', []),
  /** rooms created here that no device is in yet */
  extraRooms: read('maison-rooms-extra', []),
});

export function hideRoom(name, hide = true) {
  const rest = layout.hiddenRooms.filter((n) => n !== name);
  layout.hiddenRooms = hide ? [...rest, name] : rest;
  keep('maison-rooms-hidden', layout.hiddenRooms);
}

/** A new, empty room. Returns its name, or null when it cannot be. */
export function addRoom(raw) {
  const name = String(raw ?? '').trim().replace(/\s+/g, ' ').slice(0, 40);
  if (!name) return null;
  const known = [...roomList().map((r) => r.name), ...(home.config?.rooms ?? []).map((r) => r.name), ...layout.extraRooms];
  const same = known.find((n) => n.toLowerCase() === name.toLowerCase());
  if (same) {
    hideRoom(same, false);
    return same;
  }
  hideRoom(name, false);
  layout.extraRooms = [...layout.extraRooms, name];
  keep('maison-rooms-extra', layout.extraRooms);
  return name;
}

/** Puts a device in a room (null: out of any room the family named). */
export async function setDeviceRoom(id, room) {
  try {
    const res = await fetch(`/api/labels/${encodeURIComponent(id)}`, {
      method: 'PUT',
      headers: { 'content-type': 'application/json', 'x-moli-origin': 'ui' },
      body: JSON.stringify({ room: room ?? '' }),
    });
    if (!res.ok) {
      const body = await res.json().catch(() => ({}));
      throw new Error(body.error ?? res.statusText);
    }
    return true;
  } catch (err) {
    note(t('maison.equipements.echec', { error: err.message }), 'error');
    return false;
  }
}

/** Takes a room away: its devices lose the family's label; a room the
 *  bridges still name (or the layout file) is hidden instead. */
export async function removeRoom(name, devices) {
  for (const d of devices) await setDeviceRoom(d.id, null);
  layout.extraRooms = layout.extraRooms.filter((n) => n !== name);
  keep('maison-rooms-extra', layout.extraRooms);
  // A bridge may still name the room, and the layout file does: hide it.
  if (devices.length || (home.config?.rooms ?? []).some((r) => r.name === name)) {
    hideRoom(name, true);
    note(t('maison.pieces.retiree', { name }));
  }
}

// ---- which devices the dashboard shows -------------------------------------------

/** Devices the family took off the dashboard / put on it, against the default
 *  (a clim shows unless it was taken off). Kept like the tiles. */
export const shown = $state({ off: read('maison-dashboard-off', []), on: read('maison-dashboard-on', []) });

/** Whether a device is on the dashboard: its own choice, else `byDefault`. */
export function onDashboard(id, byDefault = true) {
  if (shown.off.includes(id)) return false;
  if (shown.on.includes(id)) return true;
  return byDefault;
}

export function showOnDashboard(id, yes) {
  shown.off = yes ? shown.off.filter((i) => i !== id) : [...new Set([...shown.off, id])];
  shown.on = yes ? [...new Set([...shown.on, id])] : shown.on.filter((i) => i !== id);
  keep('maison-dashboard-off', shown.off);
  keep('maison-dashboard-on', shown.on);
}

// ---- the dashboard's tiles -------------------------------------------------------

/** Width in columns (of 12), height in rows of `ROW` px (0: its own). */
export const ROW = 60;

export const dash = $state({ tiles: read('maison-dashboard', {}) });

export function tileOf(id, fallback) {
  return { ...fallback, ...(dash.tiles[id] ?? {}) };
}

export function setTile(id, patch, save = true) {
  dash.tiles = { ...dash.tiles, [id]: { ...(dash.tiles[id] ?? {}), ...patch } };
  if (save) keep('maison-dashboard', dash.tiles);
}

/** Puts the tiles in this order (the first of `ids` is the first shown). */
export function reorder(ids) {
  const next = { ...dash.tiles };
  ids.forEach((id, i) => (next[id] = { ...(next[id] ?? {}), order: i }));
  dash.tiles = next;
  keep('maison-dashboard', dash.tiles);
}

export function saveDashboard() {
  keep('maison-dashboard', dash.tiles);
}

export function resetDashboard() {
  dash.tiles = {};
  keep('maison-dashboard', dash.tiles);
}

// ---- what the house kept ---------------------------------------------------------

const list = (v) => (Array.isArray(v) ? v : null);

// Once the house's choices arrive (they win over what this device had).
onPrefs((kept) => {
  layout.hiddenRooms = list(kept['maison-rooms-hidden']) ?? layout.hiddenRooms;
  layout.extraRooms = list(kept['maison-rooms-extra']) ?? layout.extraRooms;
  shown.off = list(kept['maison-dashboard-off']) ?? shown.off;
  shown.on = list(kept['maison-dashboard-on']) ?? shown.on;
  const tiles = kept['maison-dashboard'];
  if (tiles && typeof tiles === 'object' && !Array.isArray(tiles)) dash.tiles = tiles;
});
